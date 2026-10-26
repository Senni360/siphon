#requires -Version 5.1
[CmdletBinding()]
param([string]$BinaryDirectory, [switch]$BootstrapDownloader, [string]$InstallerDirectory)
$ErrorActionPreference = 'Stop'
$installer = Split-Path -Parent $PSScriptRoot
if ($InstallerDirectory) { $installer = (Resolve-Path -LiteralPath $InstallerDirectory).Path }
. (Join-Path $installer 'common.ps1')
function Assert-InstallTest([bool]$Condition, [string]$Message) {
    if (-not $Condition) { throw $Message }
    Write-Host ('PASS: ' + $Message)
}
$testId = [guid]::NewGuid().ToString('N')
$temporary = Join-Path $env:TEMP ('Siphon Rust test % & ! space ' + $testId)
$registryRoot = 'HKCU:\Software\Siphon-Installer-Tests\' + $testId
$originalLocal = $env:LOCALAPPDATA
$originalPath = $env:PATH
$originalFail = $env:SIPHON_INSTALLER_TEST_FAIL
$originalPython = $env:PYTHONHOME
$realBinaries = -not [string]::IsNullOrEmpty($BinaryDirectory)
$shell = (Get-Process -Id $PID).Path
$productionRegistrations = @{}
foreach ($key in @(Get-SiphonRegistryPaths)) {
    $productionRegistrations[$key] = if (Test-Path -LiteralPath $key) { (Get-Item -LiteralPath $key).GetValue('') } else { $null }
}
try {
    New-Item -ItemType Directory -Path $temporary -Force | Out-Null
    $env:LOCALAPPDATA = Join-Path $temporary 'local'
    New-Item -ItemType Directory -Path $env:LOCALAPPDATA -Force | Out-Null
    $stub = Join-Path $temporary 'stub.exe'
    & powershell.exe -NoProfile -ExecutionPolicy Bypass -File (Join-Path $PSScriptRoot 'New-StubHelper.ps1') -OutputPath $stub
    if ($LASTEXITCODE -ne 0) { throw 'Compiling the test fixture failed.' }
    if (-not $realBinaries) {
        $BinaryDirectory = Join-Path $temporary 'binaries'
        New-Item -ItemType Directory -Path $BinaryDirectory -Force | Out-Null
        foreach ($name in @('siphon-host.exe', 'siphon-tray.exe', 'ffmpeg.exe')) {
            Copy-Item -LiteralPath $stub -Destination (Join-Path $BinaryDirectory $name)
        }
        $env:PATH = $BinaryDirectory + ';' + $env:PATH
    }
    $installation = Join-Path $env:LOCALAPPDATA 'Siphon'
    $legacyHost = Join-Path $installation 'host'
    New-Item -ItemType Directory -Path $legacyHost -Force | Out-Null
    Write-SiphonUtf8 (Join-Path $legacyHost 'keep.txt') 'preserved Python fallback'
    $legacyLauncher = Join-Path $installation 'siphon-host.cmd'
    Write-SiphonUtf8 $legacyLauncher '@rem preserved legacy launcher'
    Register-SiphonHost -ManifestPath $legacyLauncher -RegistryRoot $registryRoot
    $script:injectedFailure = $false
    function Set-Item {
        [CmdletBinding()]param([string]$LiteralPath, [object]$Value)
        if (-not $script:injectedFailure -and $LiteralPath -like '*Microsoft\Edge*') {
            $script:injectedFailure = $true
            throw 'Injected registry write failure'
        }
        Microsoft.PowerShell.Management\Set-Item @PSBoundParameters
    }
    try {
        $rejected = $false
        try { Register-SiphonHost -ManifestPath 'broken-manifest' -RegistryRoot $registryRoot }
        catch { $rejected = $_.Exception.Message -eq 'Injected registry write failure' }
        Assert-InstallTest $rejected 'Partial browser registration failure is surfaced'
        foreach ($key in @(Get-SiphonRegistryPaths $registryRoot)) {
            Assert-InstallTest ((Get-Item -LiteralPath $key).GetValue('') -eq $legacyLauncher) 'Partial registration restores previous values'
        }
    }
    finally { Remove-Item -LiteralPath Function:\Set-Item }
    if ($realBinaries -and -not $BootstrapDownloader) {
        $managed = Join-Path $originalLocal 'Siphon\bin\yt-dlp'
        if (-not (Test-Path -LiteralPath (Join-Path $managed 'yt-dlp.exe'))) {
            throw 'No managed yt-dlp to copy. Use -BootstrapDownloader for an isolated real download.'
        }
        New-Item -ItemType Directory -Path (Join-Path $installation 'bin') -Force | Out-Null
        Copy-Item -LiteralPath $managed -Destination (Join-Path $installation 'bin\yt-dlp') -Recurse
    }
    # Invalid Python environment proves Rust setup does not invoke system Python.
    $env:PYTHONHOME = Join-Path $temporary 'no-python-here'
    $installArgs = @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', (Join-Path $installer 'install.ps1'),
        '-Yes', '-RegistryRoot', $registryRoot)
    if (-not $InstallerDirectory) { $installArgs += @('-BinaryDirectory', $BinaryDirectory) }
    & $shell @installArgs | Out-Host
    Assert-InstallTest ($LASTEXITCODE -eq 0) 'Rust installation succeeds with an invalid Python environment'
    $keys = @(Get-SiphonRegistryPaths $registryRoot)
    $firstManifest = (Get-Item -LiteralPath $keys[0]).GetValue('')
    $firstRelease = Split-Path -Parent $firstManifest
    Assert-InstallTest ($firstManifest.StartsWith((Join-Path $installation 'releases'))) 'Registration points to an immutable staged release'
    foreach ($key in $keys) { Assert-InstallTest ((Get-Item -LiteralPath $key).GetValue('') -eq $firstManifest) 'All four browsers activate the tested release' }
    Assert-InstallTest (Test-Path -LiteralPath (Join-Path $firstRelease 'siphon-tray.exe')) 'Tray packaged beside host after probe'
    Assert-InstallTest (-not (Test-Path -LiteralPath (Join-Path $env:LOCALAPPDATA 'tray-started'))) 'Installation does not start the tray'
    Assert-InstallTest ((Get-Content -LiteralPath (Join-Path $legacyHost 'keep.txt') -Raw) -eq 'preserved Python fallback') 'Existing Python host preserved'
    Assert-InstallTest ((Get-Content -LiteralPath $legacyLauncher -Raw) -eq '@rem preserved legacy launcher') 'Legacy launcher preserved'
    & $shell @installArgs | Out-Host
    Assert-InstallTest ($LASTEXITCODE -eq 0) 'A repeat install succeeds without overwriting the previous release'
    $secondManifest = (Get-Item -LiteralPath $keys[0]).GetValue('')
    Assert-InstallTest ($secondManifest -ne $firstManifest -and (Test-Path -LiteralPath $firstManifest)) 'Prior release retained after activation'

    $badBinaries = Join-Path $temporary 'bad-binaries'
    New-Item -ItemType Directory -Path $badBinaries -Force | Out-Null
    Copy-Item -LiteralPath $stub -Destination (Join-Path $badBinaries 'siphon-host.exe')
    Copy-Item -LiteralPath $stub -Destination (Join-Path $badBinaries 'siphon-tray.exe')
    $env:SIPHON_INSTALLER_TEST_FAIL = '1'
    $ErrorActionPreference = 'Continue'
    & $shell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $installer 'install.ps1') -Yes -BinaryDirectory $badBinaries -RegistryRoot $registryRoot 2>&1 | ForEach-Object { Write-Host $_ }
    $failedExit = $LASTEXITCODE
    $ErrorActionPreference = 'Stop'
    Assert-InstallTest ($failedExit -ne 0) 'A host with missing ffmpeg is rejected before registration'
    $env:SIPHON_INSTALLER_TEST_FAIL = $originalFail
    foreach ($key in $keys) { Assert-InstallTest ((Get-Item -LiteralPath $key).GetValue('') -eq $secondManifest) 'Failed install keeps the active registration' }
    Assert-InstallTest (@(Get-ChildItem -LiteralPath (Join-Path $installation 'releases') -Directory).Count -eq 2) 'Failed staging directory cleaned up'

    $fallbackScripts = Join-Path $legacyHost '.venv\Scripts'
    New-Item -ItemType Directory -Path $fallbackScripts -Force | Out-Null
    Copy-Item -LiteralPath $stub -Destination (Join-Path $fallbackScripts 'python.exe')
    & $shell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $installer 'install.ps1') -Yes -Engine Python -RegistryRoot $registryRoot | Out-Host
    Assert-InstallTest ($LASTEXITCODE -eq 0) 'Explicit Python fallback reuses the preserved environment without pip'
    $fallbackManifest = (Get-Item -LiteralPath $keys[0]).GetValue('')
    Assert-InstallTest ((Split-Path -Leaf (Split-Path -Parent $fallbackManifest)) -like 'python-*') 'Python fallback selected only when requested'
    Assert-InstallTest (Test-Path -LiteralPath $secondManifest) 'Switching to Python retains Rust release'

    & $shell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $installer 'uninstall.ps1') -Yes -RegistryRoot $registryRoot | Out-Host
    Assert-InstallTest ($LASTEXITCODE -eq 0 -and -not (Test-Path -LiteralPath $installation)) 'Uninstall removes isolated Rust releases and fallback'
    foreach ($key in $keys) { Assert-InstallTest (-not (Test-Path -LiteralPath $key)) 'Uninstall removes isolated registration' }
    foreach ($key in $productionRegistrations.Keys) {
        $current = if (Test-Path -LiteralPath $key) { (Get-Item -LiteralPath $key).GetValue('') } else { $null }
        Assert-InstallTest ($current -eq $productionRegistrations[$key]) 'Production registration untouched'
    }
}
finally {
    $env:LOCALAPPDATA = $originalLocal
    $env:PATH = $originalPath
    $env:SIPHON_INSTALLER_TEST_FAIL = $originalFail
    $env:PYTHONHOME = $originalPython
    if (Test-Path -LiteralPath $registryRoot) { Remove-Item -LiteralPath $registryRoot -Recurse -Force }
    Remove-SiphonChildDirectory -Path $temporary -Parent $env:TEMP
}
