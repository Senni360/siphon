function Install-SiphonFfmpeg {
    param([string]$InstallDirectory, [switch]$Yes)
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
