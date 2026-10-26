# Shared by the source installer and release packager. No Python required.
function Build-SiphonRust {
    param([string]$Repository, [switch]$Dev)
    if (-not (Get-Command cargo.exe -ErrorAction SilentlyContinue)) {
        throw 'Rust build tools are missing. Use a packaged release, install Rust with MSVC, or explicitly select -Engine Python.'
    }
    $target = 'x86_64-pc-windows-msvc'
    $profile = 'release'
    $arguments = @('build', '--locked', '--manifest-path', (Join-Path $Repository 'helper\Cargo.toml'),
        '--target', $target, '--target-dir', (Join-Path $Repository 'helper\target\distribution'), '-p', 'siphon-host', '-p', 'siphon-tray')
    if ($Dev) { $profile = 'debug' } else { $arguments += '--release' }
    # Distributable helpers must not need a separately installed VC runtime.
    $oldFlags = $env:RUSTFLAGS
    $oldEncodedFlags = $env:CARGO_ENCODED_RUSTFLAGS
    try {
        $env:CARGO_ENCODED_RUSTFLAGS = $null
        $env:RUSTFLAGS = ($oldFlags + ' -C target-feature=+crt-static').Trim()
        & cargo.exe @arguments | Out-Host
        if ($LASTEXITCODE -ne 0) { throw 'Building the Rust helper failed. Check the MSVC C++ build tools and Windows SDK.' }
    }
    finally { $env:RUSTFLAGS = $oldFlags; $env:CARGO_ENCODED_RUSTFLAGS = $oldEncodedFlags }
    return Join-Path $Repository ('helper\target\distribution\' + $target + '\' + $profile)
}

function Assert-SiphonRustBinaries {
    param([string]$BinaryDirectory)
    foreach ($name in @('siphon-host.exe', 'siphon-tray.exe')) {
        if (-not (Test-Path -LiteralPath (Join-Path $BinaryDirectory $name) -PathType Leaf)) {
            throw ('Missing ' + $name + ' in ' + $BinaryDirectory + '. Build both binaries or use a complete package.')
        }
    }
}

function Install-SiphonYtdlp {
    param([string]$LauncherPath, [string]$InstallDirectory)
    $executable = Join-Path $InstallDirectory 'bin\yt-dlp\yt-dlp.exe'
    if (Test-Path -LiteralPath $executable) {
        & $executable --version | Out-Host
        if ($LASTEXITCODE -eq 0) { return }
        throw 'The managed yt-dlp cannot run. Repair or remove Siphon\bin\yt-dlp and rerun the installer.'
    }
    Write-Host 'Installing the standalone yt-dlp bundle using the Rust checksum-verified updater...'
    $result = Invoke-SiphonHostRequest -LauncherPath $LauncherPath -TimeoutMs 900000 -RequestJson '{"id":"install-ytdlp","type":"update"}'
    if (-not (Test-Path -LiteralPath $executable) -or -not $result.applied) {
        throw 'yt-dlp was staged but could not be installed. Finish active downloads and retry.'
    }
}
