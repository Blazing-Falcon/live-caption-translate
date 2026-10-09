# One-time setup for a fresh clone: checks prerequisites, then downloads and verifies the pinned
# native libraries, llama-server, npm packages and Playwright's browser into .deps/ (or LT_DEPS_DIR).
#   -Models     also download the models into models/ (lt-cli models fetch)
#   -TestAudio  also fetch the public test clips into testdata/fetched/ (needs an ~850 MB archive)
[CmdletBinding()]
param([switch]$Models, [switch]$TestAudio)
$ErrorActionPreference = 'Stop'
. "$PSScriptRoot/env.ps1"
Push-Location $ltRoot
try {
    Write-Host '== Prerequisites' -ForegroundColor Cyan
    $ltProblems = @()

    $ltToolchain = (Get-Content -Raw 'rust-toolchain.toml' | Select-String 'channel\s*=\s*"([^"]+)"').Matches[0].Groups[1].Value
    if (-not (Get-Command rustup -ErrorAction SilentlyContinue)) {
        $ltProblems += "Rust: rustup not found. Install it from https://rustup.rs (rust-toolchain.toml pins Rust $ltToolchain)."
    } else {
        # rustup installs the pinned toolchain on first use inside this repository.
        $ltRustc = (& rustc --version 2>&1 | Out-String).Trim()
        if ($LASTEXITCODE -ne 0 -or $ltRustc -notmatch [regex]::Escape("rustc $ltToolchain")) {
            $ltProblems += "Rust: expected rustc $ltToolchain, got '$ltRustc'. Run: rustup toolchain install $ltToolchain"
        } else { Write-Host "ok  $ltRustc" }
    }

    $ltNodeWanted = [version]((Get-Content -Raw 'app/ui/package.json' | ConvertFrom-Json).engines.node -replace '[^0-9.]', '')
    if (-not (Get-Command node -ErrorAction SilentlyContinue)) {
        $ltProblems += "Node.js: not found. Install Node $ltNodeWanted or newer (see .nvmrc)."
    } else {
        $ltNode = [version]((& node --version).TrimStart('v'))
        if ($ltNode -lt $ltNodeWanted) { $ltProblems += "Node.js: $ltNode is too old; $ltNodeWanted or newer is required (see .nvmrc)." }
        else { Write-Host "ok  Node $ltNode" }
    }

    $ltPython = Find-LtPython
    if (-not $ltPython) { $ltProblems += 'Python: 3.11 or newer not found. Install it from https://www.python.org, or set LT_PYTHON to an interpreter.' }
    else { Write-Host "ok  Python ($($ltPython -join ' '))" }

    $ltVsWhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio/Installer/vswhere.exe'
    $ltVc = if (Test-Path -LiteralPath $ltVsWhere) {
        & $ltVsWhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
    }
    if (-not $ltVc) { $ltProblems += 'Visual Studio Build Tools: the "Desktop development with C++" workload (MSVC x64 and a Windows SDK) is missing. Install it from https://visualstudio.microsoft.com/downloads/.' }
    else { Write-Host "ok  MSVC build tools ($ltVc)" }

    $ltWebView = foreach ($ltKey in 'HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}',
        'HKCU:\SOFTWARE\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}') {
        (Get-ItemProperty -LiteralPath $ltKey -ErrorAction SilentlyContinue).pv
    }
    $ltWebView = @($ltWebView | Where-Object { $_ -and $_ -ne '0.0.0.0' })
    if (-not $ltWebView) { Write-Host 'warn WebView2 Runtime not found. Builds and tests work without it; running the app needs it (https://developer.microsoft.com/microsoft-edge/webview2/).' -ForegroundColor Yellow }
    else { Write-Host "ok  WebView2 $($ltWebView[0])" }

    if ($ltProblems) {
        $ltProblems | ForEach-Object { Write-Host "missing  $_" -ForegroundColor Red }
        throw 'Install the missing prerequisites above, then run scripts/setup.ps1 again.'
    }

    Write-Host '== Native runtime (sherpa-onnx without TTS + ONNX Runtime)' -ForegroundColor Cyan
    Invoke-LtPython scripts/prepare-native-runtime.py

    Write-Host '== llama-server' -ForegroundColor Cyan
    Write-Host (Join-Path (Install-LtLlamaServer) 'llama-server.exe')

    Write-Host '== UI packages' -ForegroundColor Cyan
    npm --prefix app/ui ci --no-audit --no-fund
    if ($LASTEXITCODE -ne 0) { throw 'npm ci failed' }

    Write-Host "== Playwright's browser" -ForegroundColor Cyan
    Push-Location app/ui
    try {
        node node_modules/@playwright/test/cli.js install chromium
        if ($LASTEXITCODE -ne 0) { throw 'Playwright browser install failed' }
    } finally { Pop-Location }

    if ($Models) {
        Write-Host '== Models' -ForegroundColor Cyan
        . "$PSScriptRoot/env.ps1"
        cargo run -p lt-cli -- models fetch --dir models
        if ($LASTEXITCODE -ne 0) { throw 'Model download failed' }
    }
    if ($TestAudio) {
        Write-Host '== Test audio' -ForegroundColor Cyan
        $ltVenv = Join-Path $ltDeps 'venv'
        if (-not (Test-Path -LiteralPath (Join-Path $ltVenv 'Scripts/python.exe'))) { Invoke-LtPython -m venv $ltVenv }
        & (Join-Path $ltVenv 'Scripts/python.exe') -m pip install --quiet huggingface_hub pyarrow soundfile numpy
        if ($LASTEXITCODE -ne 0) { throw 'pip install failed' }
        & (Join-Path $ltVenv 'Scripts/python.exe') scripts/fetch-test-audio.py --out testdata/fetched
        if ($LASTEXITCODE -ne 0) { throw 'Test audio download failed' }
    }
    Write-Host 'Setup complete. Next: scripts/check.ps1' -ForegroundColor Green
} finally {
    Pop-Location
}
