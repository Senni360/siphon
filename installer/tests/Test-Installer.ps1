#requires -Version 5.1
$ErrorActionPreference = 'Stop'
. (Join-Path (Split-Path -Parent $PSScriptRoot) 'common.ps1')

function Assert-SiphonTest {
    param([bool]$Condition, [string]$Message)
    if (-not $Condition) { throw $Message }
    Write-Host ('PASS: ' + $Message)
}

$testId = [guid]::NewGuid().ToString('N')
$testDirectory = Join-Path $env:TEMP ('Siphon test % space ' + $testId)
$registryRoot = 'HKCU:\Software\Siphon-Installer-Tests\' + $testId
try {
    New-Item -ItemType Directory -Path $testDirectory -Force | Out-Null
    $launcher = Join-Path $testDirectory 'siphon-host.cmd'
    $manifestPath = Join-Path $testDirectory 'com.siphon.host.json'
    New-SiphonManifest $launcher $manifestPath
    $manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
    Assert-SiphonTest ($manifest.name -eq 'com.siphon.host' -and $manifest.type -eq 'stdio') 'Manifest host name and stdio transport'
    Assert-SiphonTest ($manifest.path -eq $launcher -and [IO.Path]::IsPathRooted($manifest.path)) 'Manifest launcher is an absolute CMD path'
    Assert-SiphonTest ($manifest.allowed_origins.Count -eq 1 -and $manifest.allowed_origins[0] -eq 'chrome-extension://bieofegakabfdjplcnpiaegdifmempmf/') 'Manifest allows only the pinned extension'
    $bytes = [IO.File]::ReadAllBytes($manifestPath)
    Assert-SiphonTest ($bytes[0] -eq 123) 'Manifest JSON is UTF-8 without BOM'

    $keys = @(Get-SiphonRegistryPaths $registryRoot)
    $suffixes = @('Chromium', 'Google\Chrome', 'Microsoft\Edge', 'BraveSoftware\Brave-Browser')
    Assert-SiphonTest ($keys.Count -eq 4) 'Exactly four browser registrations'
    foreach ($vendor in $suffixes) {
        Assert-SiphonTest ($keys -contains ($registryRoot + '\' + $vendor + '\NativeMessagingHosts\com.siphon.host')) ('Registry path: ' + $vendor)
    }
    # Exercise actual registration twice under an isolated HKCU test subtree only.
    Register-SiphonHost -ManifestPath $manifestPath -RegistryRoot $registryRoot
    Register-SiphonHost -ManifestPath $manifestPath -RegistryRoot $registryRoot
    foreach ($key in $keys) {
        Assert-SiphonTest ((Get-Item -LiteralPath $key).GetValue('') -eq $manifestPath) ('Idempotent default value: ' + $key)
    }
    Unregister-SiphonHost -RegistryRoot $registryRoot
    Unregister-SiphonHost -RegistryRoot $registryRoot
    foreach ($key in $keys) { Assert-SiphonTest (-not (Test-Path -LiteralPath $key)) 'Isolated unregister is idempotent' }

    $python = Find-SiphonPython
    $package = Join-Path $testDirectory 'siphon_host'
    New-Item -ItemType Directory -Path $package -Force | Out-Null
    Write-SiphonUtf8 (Join-Path $package '__init__.py') ''
    $stub = @'
import json, struct, sys
header = sys.stdin.buffer.read(4)
length = struct.unpack("<I", header)[0]
assert 2 <= length <= 1048576, header.hex()
request = json.loads(sys.stdin.buffer.read(length))
assert request == {"id": "t", "type": "hello", "extVersion": "installer"}, request
reply = {"id": "t", "type": "result", "ok": True, "data": {"hostVersion": "stub-\u2603", "ytdlpVersion": "test", "ffmpeg": {"found": True, "version": "test", "path": "test"}, "jsRuntime": None}}
payload = json.dumps(reply, ensure_ascii=False).encode("utf-8")
sys.stdout.buffer.write(struct.pack("<I", len(payload)) + payload)
sys.stdout.buffer.flush()
'@
    Write-SiphonUtf8 (Join-Path $package '__main__.py') $stub
    New-SiphonLauncher -TemplatePath (Join-Path (Split-Path -Parent $PSScriptRoot) 'launcher\siphon-host.cmd') -LauncherPath $launcher -HostDirectory $testDirectory -PythonPath $python
    $hello = Test-SiphonHost $launcher
    Assert-SiphonTest ($hello.hostVersion -eq ('stub-' + [char]0x2603)) 'CMD launcher preserves binary framing and UTF-8 (path has spaces and percent)'

    # Always cover the actual Windows PowerShell 5.1 runtime, even when Pester
    # itself runs in PowerShell 7. Also cover 7 when installed, in fresh processes.
    $compatibilityTest = Join-Path $PSScriptRoot 'Test-HostCompatibility.ps1'
    foreach ($shellName in @('powershell.exe', 'pwsh.exe')) {
        $shell = Get-Command $shellName -ErrorAction SilentlyContinue
        if (-not $shell) {
            if ($shellName -eq 'powershell.exe') { throw 'Windows PowerShell 5.1 is required for installer compatibility tests.' }
            continue
        }
        & $shell.Source -NoProfile -ExecutionPolicy Bypass -File $compatibilityTest -LauncherPath $launcher | ForEach-Object { Write-Host $_ }
        Assert-SiphonTest ($LASTEXITCODE -eq 0) ($shellName + ' real self-test subprocess exits successfully')
    }

    Write-SiphonUtf8 (Join-Path $package '__main__.py') 'import sys; sys.stdout.buffer.write(b"oops"); sys.stdout.buffer.flush()'
    $rejected = $false
    try { Test-SiphonHost $launcher | Out-Null }
    catch { $rejected = $_.Exception.Message -like '*Invalid native message length*' }
    Assert-SiphonTest $rejected 'Self-test rejects stdout contamination'

    Write-SiphonUtf8 (Join-Path $package '__main__.py') 'import time; time.sleep(30)'
    $timer = [Diagnostics.Stopwatch]::StartNew()
    $timedOut = $false
    try { Test-SiphonHost -LauncherPath $launcher -TimeoutMs 300 | Out-Null }
    catch { $timedOut = $_.Exception.Message -like '*timed out*' }
    Assert-SiphonTest ($timedOut -and $timer.Elapsed.TotalSeconds -lt 10) 'Self-test timeout terminates the child process tree'
    Write-Host 'All installer checks passed; production registrations were untouched.'
}
finally {
    if (Test-Path -LiteralPath $registryRoot) { Remove-Item -LiteralPath $registryRoot -Recurse -Force }
    $resolvedTestDirectory = [IO.Path]::GetFullPath($testDirectory)
    $expectedParent = [IO.Path]::GetFullPath($env:TEMP).TrimEnd('\') + '\'
    if (-not $resolvedTestDirectory.StartsWith($expectedParent, [StringComparison]::OrdinalIgnoreCase)) { throw 'Refusing cleanup outside TEMP.' }
    if (Test-Path -LiteralPath $resolvedTestDirectory) { Remove-Item -LiteralPath $resolvedTestDirectory -Recurse -Force }
}
