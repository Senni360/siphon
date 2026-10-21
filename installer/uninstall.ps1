#requires -Version 5.1
[CmdletBinding()]
param([switch]$Yes, [string]$RegistryRoot = 'HKCU:\Software')

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'common.ps1')
try {
    $installDirectory = [IO.Path]::GetFullPath((Join-Path $env:LOCALAPPDATA 'Siphon'))
    $expectedDirectory = [IO.Path]::Combine([IO.Path]::GetFullPath($env:LOCALAPPDATA), 'Siphon')
    if ($installDirectory -ne $expectedDirectory -or (Split-Path -Leaf $installDirectory) -ne 'Siphon') {
        throw 'Refusing to remove an unexpected install directory.'
    }
    if (-not $Yes) {
        $answer = Read-Host ('Unregister Siphon and remove ' + $installDirectory + '? Downloaded music in your Music folder is kept. [y/N]')
        if ($answer -notmatch '^(y|yes)$') { Write-Host 'Uninstall cancelled.'; return }
    }
    Unregister-SiphonHost -RegistryRoot $RegistryRoot
    if (Test-Path -LiteralPath $installDirectory) { Remove-Item -LiteralPath $installDirectory -Recurse -Force }
    Write-Host 'Siphon host uninstalled. Downloaded music was kept. Remove the extension from helium://extensions if desired.'
    Write-Host 'Development-mode host files and the repository were kept.'
}
catch {
    Write-Error ('Siphon uninstall failed: ' + $_.Exception.Message) -ErrorAction Continue
    exit 1
}
