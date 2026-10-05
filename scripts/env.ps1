# Dot-source from the repository root: . scripts/env.ps1
# Points native builds at the runtime that setup.ps1 prepared and puts target/debug on PATH.
# Downloaded tools live in $env:LT_DEPS_DIR, or .deps in this repository when it is unset.
$ltRoot = Split-Path -Parent $PSScriptRoot
$ltDeps = if ($env:LT_DEPS_DIR) { [IO.Path]::GetFullPath($env:LT_DEPS_DIR) } else { Join-Path $ltRoot '.deps' }
$ltNativeLib = Join-Path $ltDeps 'native/sherpa-1.13.8-no-tts-ort-1.30.0/lib'
$ltLlamaDir = Join-Path $ltDeps 'llama-b11429'
$env:PYTHONUTF8 = '1'
if (Test-Path -LiteralPath $ltNativeLib -PathType Container) {
    $env:SHERPA_ONNX_LIB_DIR = $ltNativeLib
}
$ltCargoRuntime = Join-Path $ltRoot 'target/debug'
if (-not (($env:PATH -split ';') -contains $ltCargoRuntime)) {
    $env:PATH = $ltCargoRuntime + ';' + $env:PATH
}

# Python 3.11+: $env:LT_PYTHON, then python, py -3, or a uv-managed interpreter.
function Find-LtPython {
    $candidates = @()
    if ($env:LT_PYTHON) { $candidates += , @($env:LT_PYTHON) }
    $candidates += , @('python')
    $candidates += , @('py', '-3')
    if (Get-Command uv -ErrorAction SilentlyContinue) {
        $uvPython = (& uv python find '>=3.11' 2>$null)
        if ($LASTEXITCODE -eq 0 -and $uvPython) { $candidates += , @($uvPython.Trim()) }
    }
    foreach ($candidate in $candidates) {
        if (-not (Get-Command $candidate[0] -ErrorAction SilentlyContinue)) { continue }
        $arguments = @($candidate | Select-Object -Skip 1)
        $version = (& $candidate[0] @arguments -c 'import sys; print(sys.version_info >= (3, 11))' 2>$null)
        if ($LASTEXITCODE -eq 0 -and $version -eq 'True') { return , $candidate }
    }
    return $null
}

# Downloads $Url to $Path once, then checks its size and SHA-256 before anything uses it.
function Get-LtVerifiedFile([string]$Url, [string]$Path, [string]$Sha256, [long]$Size) {
    if (-not (Test-Path -LiteralPath $Path)) {
        New-Item -ItemType Directory -Force -Path (Split-Path -Parent $Path) | Out-Null
        Invoke-WebRequest -Uri $Url -OutFile "$Path.part" -UseBasicParsing
        Move-Item -LiteralPath "$Path.part" -Destination $Path -Force
    }
    $item = Get-Item -LiteralPath $Path
    if ($item.Length -ne $Size) { throw "$Path has $($item.Length) bytes; expected $Size." }
    $actual = (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($actual -ne $Sha256) { throw "$Path SHA-256 mismatch: $actual" }
}

# Pinned llama.cpp release; both llama-server processes run this build.
$ltLlama = @{
    Url    = 'https://github.com/ggml-org/llama.cpp/releases/download/b11429/llama-b11429-bin-win-cpu-x64.zip'
    Name   = 'llama-b11429-bin-win-cpu-x64.zip'
    Size   = 19398918
    Sha256 = '1283323272b04cd07905816a597a0da810918102de958f4ff6f7bbaa70ed2efe'
}

function Install-LtLlamaServer {
    $zip = Join-Path $ltDeps "downloads/$($ltLlama.Name)"
    Get-LtVerifiedFile $ltLlama.Url $zip $ltLlama.Sha256 $ltLlama.Size
    if (-not (Test-Path -LiteralPath (Join-Path $ltLlamaDir 'llama-server.exe'))) {
        Expand-Archive -LiteralPath $zip -DestinationPath $ltLlamaDir -Force
    }
    return $ltLlamaDir
}

# Runs a Python script with the interpreter Find-LtPython picked; throws when it fails.
function Invoke-LtPython {
    $python = Find-LtPython
    if (-not $python) { throw 'Python 3.11 or newer was not found. Install it, or set LT_PYTHON.' }
    $prefix = @($python | Select-Object -Skip 1)
    & $python[0] @prefix @args
    if ($LASTEXITCODE -ne 0) { throw "Python failed: $args" }
}
