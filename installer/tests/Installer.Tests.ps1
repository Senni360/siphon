Describe 'Siphon native host installation' {
    It 'generates a pinned manifest, isolates registry keys, and passes binary CMD framing checks' {
        & (Join-Path $PSScriptRoot 'Test-Installer.ps1')
    }
}
