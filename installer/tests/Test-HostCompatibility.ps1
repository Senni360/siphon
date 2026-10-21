#requires -Version 5.1
param([Parameter(Mandatory = $true)][string]$LauncherPath)

$ErrorActionPreference = 'Stop'
. (Join-Path (Split-Path -Parent $PSScriptRoot) 'common.ps1')
$originalInputEncoding = [Console]::InputEncoding
try {
    # Reproduce a UTF-8 console on .NET Framework: Process.Start must not inject
    # EF BB BF before the four-byte native messaging header. No mocks here.
    [Console]::InputEncoding = New-Object Text.UTF8Encoding($true)
    $hello = Test-SiphonHost -LauncherPath $LauncherPath
    if ($hello.hostVersion -ne ('stub-' + [char]0x2603) -or $hello.ytdlpVersion -ne 'test') {
        throw 'The real native host self-test did not return the stub versions.'
    }
    if ([BitConverter]::ToString([Console]::InputEncoding.GetPreamble()) -ne 'EF-BB-BF') {
        throw 'The self-test did not restore the caller input encoding.'
    }
    Write-Host ('PASS: PowerShell ' + $PSVersionTable.PSVersion + ' real self-test with UTF-8 BOM console input; versions returned; caller encoding restored')
}
finally {
    [Console]::InputEncoding = $originalInputEncoding
}
