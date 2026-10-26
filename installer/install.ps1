#requires -Version 5.1
[CmdletBinding()]
param(
    [switch]$Yes,
    [switch]$Dev,
    [switch]$OpenBrowser,
    [ValidateSet('Rust', 'Python')][string]$Engine = 'Rust',
    [string]$BinaryDirectory,
    [string]$RegistryRoot = 'HKCU:\Software'
)

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'common.ps1')
. (Join-Path $PSScriptRoot 'dependencies.ps1')
. (Join-Path $PSScriptRoot 'rust.ps1')
$repoDirectory = Split-Path -Parent $PSScriptRoot
$installDirectory = Join-Path $env:LOCALAPPDATA 'Siphon'
$releaseRoot = Join-Path $installDirectory 'releases'
$releaseDirectory = Join-Path $releaseRoot ($Engine.ToLowerInvariant() + '-' + [guid]::NewGuid().ToString('N'))
$registered = $false

function Copy-SiphonSource {
    param([string]$Source, [string]$Destination)
    New-Item -ItemType Directory -Path $Destination -Force | Out-Null
    foreach ($item in Get-ChildItem -LiteralPath $Source -Force) {
        if ($item.Name -in @('.venv', '__pycache__', '.pytest_cache', '.git', 'tests') -or $item.Extension -eq '.pyc') { continue }
        $target = Join-Path $Destination $item.Name
        if ($item.PSIsContainer) { Copy-SiphonSource $item.FullName $target }
        else { Copy-Item -LiteralPath $item.FullName -Destination $target -Force }
    }
}

try {
    if ($Engine -eq 'Python' -and $BinaryDirectory) { throw '-BinaryDirectory applies only to the Rust engine.' }
    if ($Engine -eq 'Rust') {
        if ($BinaryDirectory) { $BinaryDirectory = (Resolve-Path -LiteralPath $BinaryDirectory).Path }
        elseif (Test-Path -LiteralPath (Join-Path $repoDirectory 'bin\siphon-host.exe')) {
            if ($Dev) { throw '-Dev requires a source checkout; a package already contains built binaries.' }
            $BinaryDirectory = Join-Path $repoDirectory 'bin'
        }
        else { $BinaryDirectory = Build-SiphonRust -Repository $repoDirectory -Dev:$Dev }
        Assert-SiphonRustBinaries $BinaryDirectory
    }
    Write-Host ('Installing Siphon (' + $Engine + ') for the current user in ' + $installDirectory)
    # Never overwrite binaries that a running host/tray might still have open.
    New-Item -ItemType Directory -Path $releaseDirectory -Force | Out-Null
    $launcherPath = Join-Path $releaseDirectory 'siphon-host.cmd'
    $manifestPath = Join-Path $releaseDirectory 'com.siphon.host.json'

    Update-SiphonPath
    $ffmpegPath = Install-SiphonFfmpeg -InstallDirectory $installDirectory -Yes:$Yes
    $extraPaths = @((Split-Path -Parent $ffmpegPath))
    $runtimeNames = @('node.exe')
    if ($Engine -eq 'Python') { $runtimeNames += 'deno.exe' }
    foreach ($name in $runtimeNames) {
        $command = Get-Command $name -ErrorAction SilentlyContinue
        if ($command) { $extraPaths += Split-Path -Parent $command.Source }
    }

    if ($Engine -eq 'Rust') {
        $hostExecutable = Join-Path $releaseDirectory 'siphon-host.exe'
        Copy-Item -LiteralPath (Join-Path $BinaryDirectory 'siphon-host.exe') -Destination $hostExecutable
        New-SiphonRustLauncher -LauncherPath $launcherPath -ExecutablePath $hostExecutable -ExtraPaths $extraPaths
        Install-SiphonYtdlp -LauncherPath $launcherPath -InstallDirectory $installDirectory
    }
    else {
        # Reuse a working installed Python fallback without pip/network.
        $hostDirectory = Join-Path $installDirectory 'host'
        $pythonPath = Join-Path $hostDirectory '.venv\Scripts\python.exe'
        $fallbackRecord = Join-Path $installDirectory 'python-fallback.json'
        if (Test-Path -LiteralPath $fallbackRecord) {
            $fallback = Get-Content -LiteralPath $fallbackRecord -Raw | ConvertFrom-Json
            $hostDirectory = $fallback.hostDirectory
            $pythonPath = $fallback.pythonPath
        }
        if ($Dev -or -not (Test-Path -LiteralPath $pythonPath)) {
            $sourceHost = Join-Path $repoDirectory 'host'
            if (-not (Test-Path -LiteralPath (Join-Path $sourceHost 'siphon_host\__main__.py'))) {
                throw 'Python fallback is not installed. Use -Engine Python from a complete source checkout with Python 3.10+.'
            }
            $hostDirectory = Join-Path $releaseDirectory 'host'
            if ($Dev) { $hostDirectory = $sourceHost } else { Copy-SiphonSource $sourceHost $hostDirectory }
            $venvDirectory = Join-Path $hostDirectory '.venv'
            $pythonPath = Join-Path $venvDirectory 'Scripts\python.exe'
            if (-not (Test-Path -LiteralPath $pythonPath)) {
                $basePython = Find-SiphonPython
                & $basePython -m venv $venvDirectory
                if ($LASTEXITCODE -ne 0) { throw 'Creating the Python environment failed.' }
            }
            & $pythonPath -m pip install --upgrade pip
            if ($LASTEXITCODE -ne 0) { throw 'Updating pip failed.' }
            & $pythonPath -m pip install --upgrade -r (Join-Path $hostDirectory 'requirements.txt')
            if ($LASTEXITCODE -ne 0) { throw 'Installing Python host requirements failed.' }
            & $pythonPath -m pip install --upgrade 'yt-dlp[default]' yt-dlp-ejs mutagen
            if ($LASTEXITCODE -ne 0) { throw 'Installing Python downloader dependencies failed.' }
        }
        & $pythonPath -c 'import sys; sys.exit(0 if sys.version_info >= (3, 10) else 1)'
        if ($LASTEXITCODE -ne 0) { throw 'The fallback requires Python 3.10 or newer.' }
        New-SiphonLauncher -TemplatePath (Join-Path $PSScriptRoot 'launcher\siphon-host.cmd') -LauncherPath $launcherPath -HostDirectory $hostDirectory -PythonPath $pythonPath -ExtraPaths $extraPaths
    }

    New-SiphonManifest -LauncherPath $launcherPath -ManifestPath $manifestPath
    Write-Host 'Testing the staged native messaging launcher (binary stdin/stdout)...'
    $hello = Test-SiphonHost -LauncherPath $launcherPath
    if (-not $hello.ffmpeg.found) { throw 'The staged host cannot run ffmpeg.' }
    if (-not $hello.ytdlpVersion) { throw 'The staged host cannot run yt-dlp.' }
    Write-Host ('  Host:   ' + $hello.hostVersion + ' (' + $Engine + ')')
    Write-Host ('  yt-dlp: ' + $hello.ytdlpVersion)
    Write-Host ('  ffmpeg: ' + $hello.ffmpeg.version)
    if ($hello.jsRuntime) { Write-Host ('  JS:     ' + $hello.jsRuntime.name + ' ' + $hello.jsRuntime.version) }
    else { Write-Warning 'No supported JavaScript runtime was detected. Install Node.js for YouTube challenges and rerun the installer.' }

    if ($Engine -eq 'Rust') {
        # hello warms the resident tray. Copy it only after the probe, so testing
        # cannot send messages to a live user tray or leave a staged tray running.
        Copy-Item -LiteralPath (Join-Path $BinaryDirectory 'siphon-tray.exe') -Destination (Join-Path $releaseDirectory 'siphon-tray.exe')
    }
    Register-SiphonHost -ManifestPath $manifestPath -RegistryRoot $RegistryRoot
    $registered = $true
    if ($Engine -eq 'Python' -and -not $Dev) {
        try { Write-SiphonUtf8 $fallbackRecord (@{ hostDirectory = $hostDirectory; pythonPath = $pythonPath } | ConvertTo-Json) }
        catch { Write-Warning ('Installed successfully, but could not record the fallback location: ' + $_.Exception.Message) }
    }
    Write-Host 'Self-test passed. Registered com.siphon.host for Chromium, Chrome, Edge, and Brave.'
    Write-Host ('Native host manifest: ' + $manifestPath)
    Write-Host ('Load unpacked extension: ' + (Join-Path $repoDirectory 'extension'))
    Write-Host 'Open helium://extensions, enable Developer mode, and load or reload Siphon.'
    if ($Engine -eq 'Rust') { Write-Host 'Python fallback files were kept. Switch back explicitly with install.cmd -Engine Python -Yes.' }
    if ($Dev -and $Engine -eq 'Rust') { Write-Host 'Development build installed as a snapshot; rerun -Dev after source changes.' }
    if ($OpenBrowser) {
        $helium = Join-Path $env:LOCALAPPDATA 'imput\Helium\Application\chrome.exe'
        if (Test-Path -LiteralPath $helium) { Start-Process -FilePath $helium -ArgumentList 'helium://extensions' }
        else { Write-Warning 'Helium was not found. Open its extensions page manually.' }
    }
}
catch {
    $failure = $_
    if (-not $registered -and (Test-Path -LiteralPath $releaseDirectory)) {
        try { Remove-SiphonChildDirectory -Path $releaseDirectory -Parent $releaseRoot }
        catch { Write-Warning ('Could not clean the failed staging directory: ' + $_.Exception.Message) }
    }
    Write-Error ('Siphon installation failed: ' + $failure.Exception.Message) -ErrorAction Continue
    exit 1
}
