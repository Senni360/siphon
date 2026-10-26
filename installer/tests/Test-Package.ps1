#requires -Version 5.1
[CmdletBinding()]
param([Parameter(Mandatory = $true)][string]$Archive, [switch]$BootstrapDownloader)
$ErrorActionPreference = 'Stop'
. (Join-Path (Split-Path -Parent $PSScriptRoot) 'common.ps1')
$temporary = Join-Path $env:TEMP ('Siphon package test ' + [guid]::NewGuid().ToString('N'))
try {
    $Archive = (Resolve-Path -LiteralPath $Archive).Path
    $expectedArchiveHash = ((Get-Content -LiteralPath ($Archive + '.sha256') -Raw) -split '\s+')[0]
    if ((Get-FileHash -LiteralPath $Archive -Algorithm SHA256).Hash -ne $expectedArchiveHash) { throw 'Archive checksum differs.' }
    Expand-Archive -LiteralPath $Archive -DestinationPath $temporary
    $roots = @(Get-ChildItem -LiteralPath $temporary -Directory)
    if ($roots.Count -ne 1) { throw 'Package must have one root directory.' }
    $package = $roots[0].FullName
    foreach ($required in @('bin\siphon-host.exe', 'bin\siphon-tray.exe', 'install.cmd', 'uninstall.cmd',
        'installer\install.ps1', 'installer\README.md', 'extension\manifest.json', 'notices\Geist-OFL.txt')) {
        if (-not (Test-Path -LiteralPath (Join-Path $package $required))) { throw ('Missing packaged file: ' + $required) }
    }
    foreach ($relative in @('host', 'helper', 'extension\node_modules', 'extension\tests')) {
        if (Test-Path -LiteralPath (Join-Path $package $relative)) { throw ('Unexpected development files: ' + $relative) }
    }
    $count = 0
    foreach ($line in Get-Content -LiteralPath (Join-Path $package 'SHA256SUMS')) {
        if ($line -notmatch '^([0-9a-f]{64})  (.+)$') { throw 'Invalid package checksum line.' }
        $expected = $Matches[1]
        $path = Join-Path $package $Matches[2]
        if ((Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash -ne $expected) { throw ('Checksum differs: ' + $path) }
        $count++
    }
    Write-Host ('PASS: ZIP, required runtime assets, Geist notice, and ' + $count + ' file checksums verified')
    & (Join-Path $PSScriptRoot 'Test-RustInstall.ps1') -InstallerDirectory (Join-Path $package 'installer') -BinaryDirectory (Join-Path $package 'bin') -BootstrapDownloader:$BootstrapDownloader
}
finally { Remove-SiphonChildDirectory -Path $temporary -Parent $env:TEMP }
