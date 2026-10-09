# Runs every check a change must pass: formatting, clippy, Rust tests, UI checks and tests, e2e.
# Run scripts/setup.ps1 once first. -Jobs limits Cargo's parallel jobs (default: Cargo's own).
# Real-model tests run only when their environment variables are set (see docs/development.md).
[CmdletBinding()]
param([int]$Jobs = 0)
$ErrorActionPreference = 'Stop'
. "$PSScriptRoot/env.ps1"
Push-Location $ltRoot
try {
    $ltJobs = if ($Jobs -gt 0) { @('--jobs', "$Jobs") } else { @() }
    function Step([string]$Name, [scriptblock]$Command) {
        Write-Host "== $Name" -ForegroundColor Cyan
        & $Command
        if ($LASTEXITCODE -ne 0) { throw "Failed: $Name" }
    }
    if (-not $env:SHERPA_ONNX_LIB_DIR) { throw 'The native runtime is missing. Run scripts/setup.ps1 first.' }
    if (-not (Test-Path -LiteralPath 'app/ui/node_modules')) { throw 'UI packages are missing. Run scripts/setup.ps1 first.' }

    # The Tauri crate embeds app/ui/dist at compile time, so the UI is checked and built first.
    Step 'UI type check' { npm --prefix app/ui run check }
    Step 'UI token lint' { npm --prefix app/ui run lint:tokens }
    Step 'UI unit tests' { npm --prefix app/ui test }
    Step 'UI build' { npm --prefix app/ui run build }
    Step 'UI end-to-end tests' { npm --prefix app/ui run e2e }

    Step 'Script tests' { Invoke-LtPython scripts/test_prepare_native_runtime.py; $global:LASTEXITCODE = 0 }
    Step 'cargo fmt --check' { cargo fmt --all --check }
    Step 'cargo clippy' { cargo clippy --workspace --all-targets --all-features @ltJobs -- -D warnings }
    Step 'Build Rust tests' { cargo test --workspace --all-features --no-run @ltJobs }
    Step 'Copy native DLLs next to test binaries' { & "$PSScriptRoot/install-native-runtime.ps1"; $global:LASTEXITCODE = 0 }

    $ltRealModel = [ordered]@{
        'ASR and VAD parity (lt-sherpa)' = @('LT_MODELS_DIR')
        'Translation fixtures (lt-llm)'  = @('LT_LLAMA_SERVER')
        'Draft fixtures (lt-llm)'        = @('LT_LLAMA_SERVER', 'LT_DRAFT_MODEL')
    }
    foreach ($ltTest in $ltRealModel.GetEnumerator()) {
        $ltMissing = @($ltTest.Value | Where-Object { -not [Environment]::GetEnvironmentVariable($_) })
        if ($ltMissing) { Write-Host "Real-model tests skipped: $($ltTest.Key) needs $($ltMissing -join ', ')" -ForegroundColor Yellow }
    }
    Step 'Rust tests' { cargo test --workspace --all-features @ltJobs }
    Write-Host 'All checks passed.' -ForegroundColor Green
} finally {
    Pop-Location
}
