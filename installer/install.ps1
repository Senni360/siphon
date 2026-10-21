#requires -Version 5.1
[CmdletBinding()]
param(
    [switch]$Yes,
    [switch]$Dev,
    [switch]$OpenBrowser,
    [string]$RegistryRoot = 'HKCU:\Software'
)

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'common.ps1')
$repoDirectory = Split-Path -Parent $PSScriptRoot
$sourceHost = Join-Path $repoDirectory 'host'
$installDirectory = Join-Path $env:LOCALAPPDATA 'Siphon'
$hostDirectory = Join-Path $installDirectory 'host'
if ($Dev) { $hostDirectory = $sourceHost }
$venvDirectory = Join-Path $hostDirectory '.venv'
$pythonPath = Join-Path $venvDirectory 'Scripts\python.exe'

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

function Install-SiphonFfmpeg {
    $binary = Find-SiphonFfmpeg $installDirectory
    if ($binary) { return $binary }
    if (-not $Yes) {
        $answer = Read-Host 'ffmpeg is missing. Install Gyan.FFmpeg with winget (or a portable build if winget fails)? [y/N]'
        if ($answer -notmatch '^(y|yes)$') { throw 'ffmpeg is required. Install it and rerun, or use -Yes to allow installation.' }
    }
    if (Get-Command winget.exe -ErrorAction SilentlyContinue) {
        Write-Host 'Installing ffmpeg using winget...'
        try {
            & winget.exe install --id Gyan.FFmpeg --exact --source winget --scope user --silent --accept-package-agreements --accept-source-agreements --disable-interactivity | Out-Host
            if ($LASTEXITCODE -ne 0) { Write-Warning "winget exited with code $LASTEXITCODE; checking for ffmpeg before trying a portable build." }
        }
        catch { Write-Warning ('winget could not install ffmpeg: ' + $_.Exception.Message) }
        Update-SiphonPath
        $binary = Find-SiphonFfmpeg $installDirectory
        if ($binary) { return $binary }
        # WinGet may have updated aliases while this process still has an old PATH.
        $wingetPackages = Join-Path $env:LOCALAPPDATA 'Microsoft\WinGet\Packages'
        if (Test-Path -LiteralPath $wingetPackages) {
            foreach ($package in Get-ChildItem -LiteralPath $wingetPackages -Directory -Filter 'Gyan.FFmpeg*') {
                $found = Get-ChildItem -LiteralPath $package.FullName -Filter ffmpeg.exe -File -Recurse | Select-Object -First 1
                if ($found) { return $found.FullName }
            }
        }
    }
    Write-Host 'Downloading the portable ffmpeg essentials build from gyan.dev...'
    [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
    $archive = Join-Path $env:TEMP ('siphon-ffmpeg-' + [guid]::NewGuid().ToString('N') + '.zip')
    try {
        Invoke-WebRequest -UseBasicParsing -Uri 'https://www.gyan.dev/ffmpeg/builds/ffmpeg-release-essentials.zip' -OutFile $archive
        Expand-Archive -LiteralPath $archive -DestinationPath (Join-Path $installDirectory 'ffmpeg') -Force
    }
    finally { if (Test-Path -LiteralPath $archive) { Remove-Item -LiteralPath $archive -Force } }
    $binary = Find-SiphonFfmpeg $installDirectory
    if (-not $binary) { throw 'The portable ffmpeg download did not contain ffmpeg.exe. See the installation instructions in README.md.' }
    return $binary
}

try {
    if (-not (Test-Path -LiteralPath (Join-Path $sourceHost 'siphon_host\__main__.py'))) {
        throw 'host\siphon_host\__main__.py is missing. Run this installer from a complete Siphon checkout.'
    }
    Write-Host ('Installing Siphon for the current user in ' + $installDirectory)
    New-Item -ItemType Directory -Path $installDirectory -Force | Out-Null
    New-Item -ItemType Directory -Path (Join-Path $installDirectory 'logs') -Force | Out-Null
    if (-not $Dev) { Copy-SiphonSource $sourceHost $hostDirectory }
    if (-not (Test-Path -LiteralPath $pythonPath)) {
        $basePython = Find-SiphonPython
        Write-Host ('Creating Python environment with ' + $basePython)
        & $basePython -m venv $venvDirectory
        if ($LASTEXITCODE -ne 0) { throw 'Creating the Python environment failed.' }
    }
    & $pythonPath -c 'import sys; sys.exit(0 if sys.version_info >= (3, 10) else 1)'
    if ($LASTEXITCODE -ne 0) { throw 'The existing venv is older than Python 3.10. Remove its .venv folder and rerun.' }
    Write-Host 'Installing/updating host dependencies...'
    & $pythonPath -m pip install --upgrade pip
    if ($LASTEXITCODE -ne 0) { throw 'Updating pip failed. Check your internet connection and proxy settings.' }
    $requirements = Join-Path $hostDirectory 'requirements.txt'
    if (Test-Path -LiteralPath $requirements) {
        & $pythonPath -m pip install --upgrade -r $requirements
        if ($LASTEXITCODE -ne 0) { throw 'Installing host requirements failed.' }
    }
    # Keep the installer contract even when an older requirements file omits EJS.
    & $pythonPath -m pip install --upgrade 'yt-dlp[default]' yt-dlp-ejs mutagen
    if ($LASTEXITCODE -ne 0) { throw 'Installing yt-dlp, yt-dlp-ejs, and mutagen failed.' }
    Update-SiphonPath
    $ffmpegPath = Install-SiphonFfmpeg
    $extraPaths = @((Split-Path -Parent $ffmpegPath))
    $runtime = $null
    foreach ($name in @('node.exe', 'deno.exe')) {
        $command = Get-Command $name -ErrorAction SilentlyContinue
        if ($command) {
            $extraPaths += Split-Path -Parent $command.Source
            if (-not $runtime) { $runtime = $command.Source }
        }
    }
    if (-not $runtime) { Write-Warning 'No Node.js or Deno was found. YouTube challenges may fail; install Node.js or Deno and rerun the installer.' }
    $launcherPath = Join-Path $installDirectory 'siphon-host.cmd'
    $manifestPath = Join-Path $installDirectory 'com.siphon.host.json'
    New-SiphonLauncher -TemplatePath (Join-Path $PSScriptRoot 'launcher\siphon-host.cmd') -LauncherPath $launcherPath -HostDirectory $hostDirectory -PythonPath $pythonPath -ExtraPaths $extraPaths
    New-SiphonManifest -LauncherPath $launcherPath -ManifestPath $manifestPath
    Write-Host 'Testing the native messaging launcher (binary stdin/stdout)...'
    $hello = Test-SiphonHost -LauncherPath $launcherPath
    Write-Host ('  Python:  ' + (& $pythonPath --version))
    Write-Host ('  Host:    ' + $hello.hostVersion)
    Write-Host ('  yt-dlp:  ' + $hello.ytdlpVersion)
    Write-Host ('  ffmpeg:  ' + $hello.ffmpeg.version + ' (' + $hello.ffmpeg.path + ')')
    if (-not $hello.ffmpeg.found) { throw 'The host cannot find ffmpeg, despite installation. See the troubleshooting steps in README.md.' }
    if ($hello.jsRuntime) { Write-Host ('  JS:      ' + $hello.jsRuntime.name + ' ' + $hello.jsRuntime.version) }
    else { Write-Warning 'The host did not detect a supported JavaScript runtime.' }
    # Publish registration only after the launcher has answered successfully.
    Register-SiphonHost -ManifestPath $manifestPath -RegistryRoot $RegistryRoot
    Write-Host 'Self-test passed. Registered com.siphon.host for Chromium, Chrome, Edge, and Brave.'
    Write-Host ''
    Write-Host 'Next steps:'
    Write-Host '  1. Open helium://extensions (or try chrome://extensions).'
    Write-Host '  2. Enable Developer mode, then click Load unpacked.'
    Write-Host ('  3. Select: ' + (Join-Path $repoDirectory 'extension'))
    Write-Host '  4. Open a supported media page and click Siphon, or press Alt+Shift+D.'
    if ($Dev) { Write-Host ('Development mode: keep this host directory in place: ' + $sourceHost) }
    else { Write-Host 'The installed host is independent of this checkout. Keep extension/ in place while it is loaded unpacked.' }
    if ($OpenBrowser) {
        $helium = Join-Path $env:LOCALAPPDATA 'imput\Helium\Application\chrome.exe'
        if (Test-Path -LiteralPath $helium) { Start-Process -FilePath $helium -ArgumentList 'helium://extensions' }
        else { Write-Warning ('Helium was not found at ' + $helium + '. Open the extensions page manually.') }
    }
}
catch {
    Write-Error ('Siphon installation failed: ' + $_.Exception.Message) -ErrorAction Continue
    exit 1
}
