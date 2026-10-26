#requires -Version 5.1
[CmdletBinding()]
param([string]$OutputDirectory, [string]$BinaryDirectory)
$ErrorActionPreference = 'Stop'
$repository = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
. (Join-Path $repository 'installer\common.ps1')
. (Join-Path $repository 'installer\rust.ps1')
if (-not $OutputDirectory) { $OutputDirectory = Join-Path $repository 'dist' }
$OutputDirectory = [IO.Path]::GetFullPath($OutputDirectory)
if (-not $BinaryDirectory) { $BinaryDirectory = Build-SiphonRust -Repository $repository }
$BinaryDirectory = (Resolve-Path -LiteralPath $BinaryDirectory).Path
Assert-SiphonRustBinaries $BinaryDirectory
$versionLine = & (Join-Path $BinaryDirectory 'siphon-host.exe') --version
if ($LASTEXITCODE -ne 0 -or $versionLine -notmatch '^siphon-host ([0-9]+\.[0-9]+\.[0-9]+(?:[-+][a-zA-Z0-9.-]+)?) ') {
    throw 'The Rust host did not report a valid release version.'
}
$version = $Matches[1]
$packageName = 'Siphon-' + $version + '-windows-x64'
New-Item -ItemType Directory -Path $OutputDirectory -Force | Out-Null
$work = Join-Path $OutputDirectory ('.package-' + [guid]::NewGuid().ToString('N'))
$package = Join-Path $work $packageName
$archive = Join-Path $OutputDirectory ($packageName + '.zip')
$partial = Join-Path $work ($packageName + '.zip')
try {
    New-Item -ItemType Directory -Path (Join-Path $package 'bin') -Force | Out-Null
    foreach ($name in @('siphon-host.exe', 'siphon-tray.exe')) {
        Copy-Item -LiteralPath (Join-Path $BinaryDirectory $name) -Destination (Join-Path $package 'bin')
    }
    New-Item -ItemType Directory -Path (Join-Path $package 'installer\launcher') -Force | Out-Null
    foreach ($name in @('install.ps1', 'uninstall.ps1', 'common.ps1', 'dependencies.ps1', 'rust.ps1', 'README.md')) {
        Copy-Item -LiteralPath (Join-Path $repository ('installer\' + $name)) -Destination (Join-Path $package 'installer')
    }
    Copy-Item -LiteralPath (Join-Path $repository 'installer\launcher\siphon-host.cmd') -Destination (Join-Path $package 'installer\launcher')
    foreach ($name in @('install.cmd', 'uninstall.cmd')) {
        Copy-Item -LiteralPath (Join-Path $repository $name) -Destination $package
    }
    Copy-Item -LiteralPath (Join-Path $repository 'extension') -Destination $package -Recurse
    # Extension development dependencies and tests are not runtime assets.
    foreach ($relative in @('extension\node_modules', 'extension\tests', 'extension\src\popup\dev', 'extension\src\options\dev')) {
        Remove-SiphonChildDirectory -Path (Join-Path $package $relative) -Parent $package
    }
    foreach ($name in @('LICENSE', 'LICENSE.md', 'NOTICE', 'NOTICE.md')) {
        if (Test-Path -LiteralPath (Join-Path $repository $name)) {
            Copy-Item -LiteralPath (Join-Path $repository $name) -Destination $package
        }
    }
    New-Item -ItemType Directory -Path (Join-Path $package 'notices') -Force | Out-Null
    Copy-Item -LiteralPath (Join-Path $repository 'helper\crates\siphon-tray\assets\OFL.txt') -Destination (Join-Path $package 'notices\Geist-OFL.txt')
    # Copy only release documentation and notices, never helper targets or model caches.
    Copy-Item -LiteralPath (Join-Path $repository 'helper\notices') -Destination (Join-Path $package 'notices\stems') -Recurse
    New-Item -ItemType Directory -Path (Join-Path $package 'docs') -Force | Out-Null
    foreach ($name in @('stems-engine.md', 'stems-protocol.md', 'stems-extension.md')) {
        Copy-Item -LiteralPath (Join-Path $repository ('helper\' + $name)) -Destination (Join-Path $package 'docs')
    }
    $installerReadme = Join-Path $package 'installer\README.md'
    $installerText = [IO.File]::ReadAllText($installerReadme).Replace('../helper/stems-', '../docs/stems-')
    Write-SiphonUtf8 $installerReadme $installerText
    Write-SiphonUtf8 (Join-Path $package 'README.txt') @'
Siphon for Windows x64

Extract this entire archive and run install.cmd (or install.cmd -Yes).
No installed Python or Rust toolchain is required. Initial installation needs
internet for standalone yt-dlp and, if missing, ffmpeg. Install Node.js for
YouTube JavaScript challenges. Existing installed Python fallback files are kept.

Load this package's extension folder at helium://extensions (Developer mode).
Keep the extracted extension folder in place while it is loaded unpacked.
See installer/README.md for details, fallback, and uninstall commands.

Stem splitting is a separate, requested action on a completed local file.
The first split downloads and verifies the native runtime and four-stem
model; these large cached files are not bundled in this archive. No system
Python is required. First use needs internet and CPU processing can be slow,
depending on hardware and track length. Normal downloads do not split stems.
The original is preserved; vocals, drums, bass and other WAV files are saved
in a folder beside it. See docs/stems-engine.md for engine validation and
docs/stems-extension.md for queue, cancel, folder and tray controls.
Actual FL Studio multi-file drag and mixer assignment remain unverified.
'@
    $hashes = foreach ($file in Get-ChildItem -LiteralPath $package -File -Recurse | Sort-Object FullName) {
        $relative = $file.FullName.Substring($package.Length + 1).Replace('\', '/')
        (Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256).Hash.ToLowerInvariant() + '  ' + $relative
    }
    Write-SiphonUtf8 (Join-Path $package 'SHA256SUMS') (($hashes -join "`n") + "`n")
    Compress-Archive -LiteralPath $package -DestinationPath $partial -CompressionLevel Optimal
    Move-Item -LiteralPath $partial -Destination $archive -Force
    $checksum = (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant()
    Write-SiphonUtf8 ($archive + '.sha256') ($checksum + '  ' + [IO.Path]::GetFileName($archive) + "`n")
    Write-Host ('Package: ' + $archive)
    Write-Host ('SHA256:  ' + $checksum)
}
finally { Remove-SiphonChildDirectory -Path $work -Parent $OutputDirectory }
