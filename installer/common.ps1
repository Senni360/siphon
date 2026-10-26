Set-StrictMode -Version 2.0

$script:SiphonHostName = 'com.siphon.host'
$script:SiphonExtensionId = 'bieofegakabfdjplcnpiaegdifmempmf'

function Write-SiphonUtf8 {
    param([string]$Path, [string]$Text)
    [IO.File]::WriteAllText($Path, $Text, (New-Object Text.UTF8Encoding($false)))
}

function Get-SiphonRegistryPaths {
    param([string]$RegistryRoot = 'HKCU:\Software')
    foreach ($vendor in @('Chromium', 'Google\Chrome', 'Microsoft\Edge', 'BraveSoftware\Brave-Browser')) {
        Join-Path $RegistryRoot ($vendor + '\NativeMessagingHosts\' + $script:SiphonHostName)
    }
}

function Register-SiphonHost {
    param([string]$ManifestPath, [string]$RegistryRoot = 'HKCU:\Software')
    $previous = @()
    try {
        foreach ($key in @(Get-SiphonRegistryPaths $RegistryRoot)) {
            $exists = Test-Path -LiteralPath $key
            $value = $null
            if ($exists) { $value = (Get-Item -LiteralPath $key).GetValue('') }
            $previous += @{ Path = $key; Exists = $exists; Value = $value }
            New-Item -Path $key -Force -ErrorAction Stop | Out-Null
            Set-Item -LiteralPath $key -Value $ManifestPath -ErrorAction Stop
        }
    }
    catch {
        $failure = $_
        foreach ($entry in $previous) {
            if ($entry.Exists) {
                if ($null -eq $entry.Value) { Remove-ItemProperty -LiteralPath $entry.Path -Name '(default)' -ErrorAction SilentlyContinue }
                else { Set-Item -LiteralPath $entry.Path -Value $entry.Value -ErrorAction Continue }
            }
            elseif (Test-Path -LiteralPath $entry.Path) { Remove-Item -LiteralPath $entry.Path -Force -ErrorAction Continue }
        }
        throw $failure
    }
}

function Unregister-SiphonHost {
    param([string]$RegistryRoot = 'HKCU:\Software')
    foreach ($key in @(Get-SiphonRegistryPaths $RegistryRoot)) {
        if (Test-Path -LiteralPath $key) { Remove-Item -LiteralPath $key -Force -ErrorAction Stop }
    }
}

function New-SiphonManifest {
    param([string]$LauncherPath, [string]$ManifestPath)
    $manifest = [ordered]@{
        name = $script:SiphonHostName
        description = 'Siphon audio downloader native messaging host'
        path = [IO.Path]::GetFullPath($LauncherPath)
        type = 'stdio'
        allowed_origins = @('chrome-extension://' + $script:SiphonExtensionId + '/')
    }
    Write-SiphonUtf8 $ManifestPath ($manifest | ConvertTo-Json -Depth 4)
}

function New-SiphonLauncher {
    param([string]$TemplatePath, [string]$LauncherPath, [string]$HostDirectory,
          [string]$PythonPath, [string[]]$ExtraPaths = @())
    # Percent is the only expandable character inside quotes with delayed expansion disabled.
    $template = [IO.File]::ReadAllText($TemplatePath)
    $template = $template.Replace('__HOST_DIR__', $HostDirectory.Replace('%', '%%'))
    $template = $template.Replace('__PYTHON__', $PythonPath.Replace('%', '%%'))
    $template = $template.Replace('__EXTRA_PATH__', (($ExtraPaths -join ';').Replace('%', '%%')))
    Write-SiphonUtf8 $LauncherPath ($template.Replace("`r`n", "`n").Replace("`n", "`r`n"))
}

function Find-SiphonPython {
    $candidates = @()
    if (Get-Command py.exe -ErrorAction SilentlyContinue) {
        $candidates += @{ Command = 'py.exe'; Arguments = @('-3.13') }
        $candidates += @{ Command = 'py.exe'; Arguments = @('-3') }
        foreach ($minor in @(14, 12, 11, 10)) {
            $candidates += @{ Command = 'py.exe'; Arguments = @('-3.' + $minor) }
        }
    }
    foreach ($name in @('python.exe', 'python3.exe')) {
        if (Get-Command $name -ErrorAction SilentlyContinue) {
            $candidates += @{ Command = $name; Arguments = @() }
        }
    }
    foreach ($candidate in $candidates) {
        $command = $candidate.Command
        $arguments = $candidate.Arguments
        try {
            $found = @(& $command @arguments -c 'import sys; print(sys.executable) if sys.version_info >= (3, 10) else sys.exit(1)' 2>$null)
            if ($LASTEXITCODE -eq 0 -and $found.Count -gt 0 -and (Test-Path -LiteralPath ([string]$found[-1]))) {
                return [string]$found[-1]
            }
        }
        catch { continue }
    }
    throw 'Python 3.10+ was not found. Install Python 3.13 with the py launcher, then rerun this installer.'
}

function Update-SiphonPath {
    $env:Path = $env:Path + ';' + [Environment]::GetEnvironmentVariable('Path', 'Machine') + ';' + [Environment]::GetEnvironmentVariable('Path', 'User')
}

function Find-SiphonFfmpeg {
    param([string]$InstallDirectory)
    $command = Get-Command ffmpeg.exe -ErrorAction SilentlyContinue
    if ($command) { return $command.Source }
    $local = Join-Path $InstallDirectory 'ffmpeg'
    if (Test-Path -LiteralPath $local) {
        $binary = Get-ChildItem -LiteralPath $local -Filter ffmpeg.exe -Recurse -File | Select-Object -First 1
        if ($binary) { return $binary.FullName }
    }
    return $null
}

function Read-SiphonBytes {
    param([IO.Stream]$Stream, [int]$Count, [Diagnostics.Stopwatch]$Timer, [int]$TimeoutMs)
    $buffer = New-Object byte[] $Count
    $offset = 0
    while ($offset -lt $Count) {
        $remaining = $TimeoutMs - [int]$Timer.ElapsedMilliseconds
        if ($remaining -le 0) { throw 'Native host self-test timed out.' }
        $read = $Stream.ReadAsync($buffer, $offset, $Count - $offset)
        if (-not $read.Wait($remaining)) { throw 'Native host self-test timed out.' }
        if ($read.Result -eq 0) { throw 'Native host closed stdout before sending a complete reply.' }
        $offset += $read.Result
    }
    return ,$buffer
}

function Invoke-SiphonHostRequest {
    param([string]$LauncherPath, [string]$RequestJson, [int]$TimeoutMs = 45000,
          [hashtable]$Environment = @{})
    $requestObject = $RequestJson | ConvertFrom-Json
    $start = New-Object Diagnostics.ProcessStartInfo
    $start.FileName = $env:ComSpec
    $start.Arguments = '/d /s /c ""' + $LauncherPath + '""'
    $start.UseShellExecute = $false
    $start.CreateNoWindow = $true
    $start.RedirectStandardInput = $true
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
    foreach ($name in $Environment.Keys) { $start.EnvironmentVariables[$name] = [string]$Environment[$name] }
    $process = New-Object Diagnostics.Process
    $process.StartInfo = $start
    $stderr = $null
    $started = $false
    try {
        $binaryInputEncoding = New-Object Text.UTF8Encoding($false)
        $originalInputEncoding = $null
        try {
            if ($start.PSObject.Properties['StandardInputEncoding']) {
                $start.StandardInputEncoding = $binaryInputEncoding
            }
            else {
                # .NET Framework (PowerShell 5.1) has no StandardInputEncoding.
                # Process.Start creates an AutoFlush StreamWriter using Console.InputEncoding;
                # a UTF-8 BOM is sent immediately, even if we only use BaseStream below.
                # Supply a BOM-free encoding while that writer is created, then restore it.
                $originalInputEncoding = [Console]::InputEncoding
                [Console]::InputEncoding = $binaryInputEncoding
            }
            if (-not $process.Start()) { throw 'Could not start native host launcher.' }
            $started = $true
        }
        finally {
            if ($null -ne $originalInputEncoding) { [Console]::InputEncoding = $originalInputEncoding }
        }
        $stderr = $process.StandardError.ReadToEndAsync()
        $request = [Text.Encoding]::UTF8.GetBytes($RequestJson)
        $prefix = [BitConverter]::GetBytes([uint32]$request.Length)
        if (-not [BitConverter]::IsLittleEndian) { [Array]::Reverse($prefix) }
        $process.StandardInput.BaseStream.Write($prefix, 0, 4)
        $process.StandardInput.BaseStream.Write($request, 0, $request.Length)
        $process.StandardInput.BaseStream.Flush()
        $timer = [Diagnostics.Stopwatch]::StartNew()
        # Ignore any protocol log events preceding the matching result.
        do {
            $header = Read-SiphonBytes $process.StandardOutput.BaseStream 4 $timer $TimeoutMs
            if (-not [BitConverter]::IsLittleEndian) { [Array]::Reverse($header) }
            $length = [BitConverter]::ToUInt32($header, 0)
            if ($length -lt 2 -or $length -gt 1048576) { throw "Invalid native message length: $length. Check stdout for accidental logging." }
            $body = Read-SiphonBytes $process.StandardOutput.BaseStream ([int]$length) $timer $TimeoutMs
            $reply = [Text.Encoding]::UTF8.GetString($body) | ConvertFrom-Json -ErrorAction Stop
        } until ($reply.PSObject.Properties['id'] -and $reply.id -eq $requestObject.id)
        if ($reply.type -ne 'result' -or -not $reply.ok) { throw ('Host rejected ' + $requestObject.type + ': ' + ($reply | ConvertTo-Json -Depth 8 -Compress)) }
        return $reply.data
    }
    catch {
        $message = $_.Exception.Message
        if ($stderr -and $stderr.IsCompleted) { $message += "`n" + $stderr.Result }
        throw $message
    }
    finally {
        if ($started) {
            $process.StandardInput.Close()
            if (-not $process.WaitForExit(2000)) {
                # cmd.exe owns the helper child; kill the entire tree on failure/timeout.
                & taskkill.exe /PID $process.Id /T /F 2>$null | Out-Null
            }
        }
        $process.Dispose()
    }
}

function Test-SiphonHost {
    param([string]$LauncherPath, [int]$TimeoutMs = 45000)
    $hello = Invoke-SiphonHostRequest -LauncherPath $LauncherPath -TimeoutMs $TimeoutMs -RequestJson '{"id":"t","type":"hello","extVersion":"installer"}'
    if (-not $hello.PSObject.Properties['hostVersion'] -or -not $hello.hostVersion) { throw 'Hello reply did not include hostVersion.' }
    return $hello
}

function New-SiphonRustLauncher {
    param([string]$LauncherPath, [string]$ExecutablePath, [string[]]$ExtraPaths = @())
    $lines = @('@echo off', 'setlocal DisableDelayedExpansion', 'chcp 65001 >nul',
        ('set "PATH=' + (($ExtraPaths -join ';').Replace('%', '%%')) + ';%PATH%"'),
        ('"' + $ExecutablePath.Replace('%', '%%') + '"'), 'exit /b %errorlevel%')
    Write-SiphonUtf8 $LauncherPath (($lines -join "`r`n") + "`r`n")
}

function Remove-SiphonChildDirectory {
    param([string]$Path, [string]$Parent)
    $resolved = [IO.Path]::GetFullPath($Path).TrimEnd('\')
    $boundary = [IO.Path]::GetFullPath($Parent).TrimEnd('\') + '\'
    if (-not $resolved.StartsWith($boundary, [StringComparison]::OrdinalIgnoreCase)) {
        throw 'Refusing to remove a directory outside the expected parent.'
    }
    if (Test-Path -LiteralPath $resolved) { Remove-Item -LiteralPath $resolved -Recurse -Force }
}
