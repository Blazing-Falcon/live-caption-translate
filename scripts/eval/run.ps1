# Runs the scoreboard over one or more domains and scores each run.
#
#   ./scripts/eval/run.ps1 -Id base
#   ./scripts/eval/run.ps1 -Id t1-none -Domain drama,ramc -Config .agent/experiments/configs/t1-none.toml
#
# Output: .agent/experiments/runs/<Id>-<domain>/{out.jsonl,out.events.jsonl,out.words.json,score.json}
# Timing runs are realtime and must run alone (see .agent/agent-guide.md section 4).
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$Id,
    [string[]]$Domain = @('drama'),
    [string]$Config = '',
    [switch]$Fast,
    [int]$Seconds = 0
)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
. "$root/scripts/env.ps1"
Push-Location $root
try {
    $cli = Join-Path $root 'target/release/lt-cli.exe'
    if (-not (Test-Path -LiteralPath $cli)) { throw 'Build lt-cli first: cargo build --release -p lt-cli --all-features' }
    $env:LT_MODELS_DIR = Join-Path $root 'models'
    if (-not $env:LT_LLAMA_SERVER) {
        $env:LT_LLAMA_SERVER = Join-Path $root '.deps/llama-b11429/llama-server.exe'
    }
    $python = Join-Path $root '.agent/venv/Scripts/python.exe'
    $pace = if ($Fast) { 'fast' } else { 'realtime' }

    $sources = @{
        drama  = @{ Wav = '.agent/cn-translate-investigation/translate.wav'; Zh = 'testdata/eval/drama-130s.zh.txt'; En = 'testdata/eval/drama-130s.en.txt'; Names = 'testdata/eval/drama-130s.names.toml' }
        ramc   = @{ Wav = 'testdata/fetched/ramc/CTS-CN-F2F-2019-11-15-1449.wav'; Zh = ''; En = ''; Names = '' }
        leijun = @{ Wav = 'testdata/fetched/leijun/lei-jun.wav'; Zh = ''; En = ''; Names = '' }
        ascend = @{ Wav = 'testdata/fetched/ascend/clip_00.wav'; Zh = ''; En = ''; Names = '' }
        fleurs = @{ Wav = 'testdata/fetched/fleurs/fleurs-dev-50.wav'; Zh = 'testdata/fetched/fleurs/fleurs-dev-50.zh.txt'; En = ''; Names = '' }
    }

    foreach ($name in $Domain) {
        $source = $sources[$name]
        if (-not $source) { throw "Unknown domain: $name" }
        $dir = Join-Path $root ".agent/experiments/runs/$Id-$name"
        New-Item -ItemType Directory -Force -Path $dir | Out-Null
        $out = Join-Path $dir 'out.jsonl'
        $replayArgs = @('replay', (Join-Path $root $source.Wav), '--pace', $pace, '--out', $out)
        if ($Config) { $replayArgs += @('--config', (Resolve-Path $Config).Path) }
        if ($Seconds -gt 0) { $replayArgs += @('--dur', "$Seconds") }
        Write-Host "== replay $name ($pace)" -ForegroundColor Cyan
        & $cli @replayArgs
        if ($LASTEXITCODE -ne 0) { throw "Replay failed for $name" }

        $scoreArgs = @((Join-Path $root 'scripts/eval/score.py'), $out, '--label', "$Id-$name")
        if ($source.Zh -and (Test-Path -LiteralPath (Join-Path $root $source.Zh))) {
            $scoreArgs += @('--zh', (Join-Path $root $source.Zh))
        }
        if ($source.En -and (Test-Path -LiteralPath (Join-Path $root $source.En))) {
            $scoreArgs += @('--en', (Join-Path $root $source.En))
        }
        if ($source.Names -and (Test-Path -LiteralPath (Join-Path $root $source.Names))) {
            $scoreArgs += @('--names', (Join-Path $root $source.Names))
        }
        & $python -I @scoreArgs | Tee-Object -FilePath (Join-Path $dir 'score.json')
    }
}
finally {
    Pop-Location
}
