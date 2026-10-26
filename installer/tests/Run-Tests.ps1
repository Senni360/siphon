#requires -Version 5.1
$ErrorActionPreference = 'Stop'
if (Get-Module -ListAvailable -Name Pester) {
    $result = Invoke-Pester -Script (Join-Path $PSScriptRoot 'Installer.Tests.ps1') -PassThru
    if ($result.FailedCount -gt 0) { throw 'Installer Pester tests failed.' }
}
else {
    & (Join-Path $PSScriptRoot 'Test-Installer.ps1')
}
& (Join-Path $PSScriptRoot 'Test-RustInstall.ps1')
