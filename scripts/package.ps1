# Builds LiveTranslation-<version>-win-x64.zip: release app, pinned runtimes, notices, bootstrapper.
# Output goes to dist/; downloads are cached in .deps/ (or LT_DEPS_DIR).
# -Jobs limits Cargo's parallel jobs (default: Cargo's own).
[CmdletBinding()]
param(
    [string]$Version = '0.2.0',
    [int]$Jobs = 0,
    [switch]$SkipUi,
    [switch]$SkipBuild,
    [switch]$SkipBootstrapper
)
$ErrorActionPreference = 'Stop'
. "$PSScriptRoot/env.ps1"
Push-Location $ltRoot
try {
    $ltCache = Join-Path $ltDeps 'downloads'
    $ltDist = Join-Path $ltRoot 'dist'
    $ltName = "LiveTranslation-$Version-win-x64"
    $ltStage = Join-Path $ltDist $ltName
    $ltZip = Join-Path $ltDist "$ltName.zip"

    Write-Host '1/6 Native runtime (no-TTS sherpa + ONNX Runtime)'
    $ltRuntime = (Invoke-LtPython scripts/prepare-native-runtime.py --json | ConvertFrom-Json)
    $env:SHERPA_ONNX_LIB_DIR = $ltRuntime.lib_dir

    if (-not $SkipUi) {
        Write-Host '2/6 Frontend'
        npm --prefix app/ui ci --no-audit --no-fund
        if ($LASTEXITCODE -ne 0) { throw 'npm ci failed' }
        npm --prefix app/ui run build
        if ($LASTEXITCODE -ne 0) { throw 'UI build failed' }
    }
    if (-not $SkipBuild) {
        Write-Host '3/6 Release build'
        $ltJobs = if ($Jobs -gt 0) { @('--jobs', "$Jobs") } else { @() }
        cargo build --release -p live-translation @ltJobs
        if ($LASTEXITCODE -ne 0) { throw 'Release build failed' }
    }

    Write-Host '4/6 Translator sidecar'
    $ltLlamaDir = Install-LtLlamaServer

    Write-Host '5/6 WebView2 bootstrapper'
    $ltBootstrapper = Join-Path $ltCache 'MicrosoftEdgeWebview2Setup.exe'
    if (-not $SkipBootstrapper) {
        if (-not (Test-Path -LiteralPath $ltBootstrapper)) {
            Invoke-WebRequest -Uri 'https://go.microsoft.com/fwlink/p/?LinkId=2124703' -OutFile $ltBootstrapper -UseBasicParsing
        }
        $ltSignature = Get-AuthenticodeSignature -LiteralPath $ltBootstrapper
        if ($ltSignature.Status -ne 'Valid' -or $ltSignature.SignerCertificate.Subject -notmatch 'O=Microsoft Corporation') {
            Remove-Item -LiteralPath $ltBootstrapper -Force
            throw "The WebView2 bootstrapper signature is not a valid Microsoft signature ($($ltSignature.Status))."
        }
    }

    Write-Host '6/6 Assemble'
    if (Test-Path -LiteralPath $ltStage) {
        if (-not ([IO.Path]::GetFullPath($ltStage)).StartsWith([IO.Path]::GetFullPath($ltDist))) { throw 'Refusing to clean outside dist' }
        Remove-Item -LiteralPath $ltStage -Recurse -Force
    }
    New-Item -ItemType Directory -Force -Path $ltStage | Out-Null
    Copy-Item -LiteralPath (Join-Path $ltRoot 'target/release/live-translation.exe') -Destination $ltStage
    foreach ($ltDll in 'sherpa-onnx-c-api.dll', 'onnxruntime.dll', 'onnxruntime_providers_shared.dll') {
        Copy-Item -LiteralPath (Join-Path $ltRuntime.lib_dir $ltDll) -Destination $ltStage
    }
    $ltServerFiles = @('llama-server.exe', 'llama-server-impl.dll', 'llama-common.dll', 'llama.dll', 'mtmd.dll', 'libomp.dll', 'LICENSE-LLVM-OpenMP') +
        (Get-ChildItem -LiteralPath $ltLlamaDir -Filter 'ggml*.dll' -File | Where-Object { $_.Name -ne 'ggml-rpc.dll' } | ForEach-Object Name)
    foreach ($ltFile in $ltServerFiles) {
        Copy-Item -LiteralPath (Join-Path $ltLlamaDir $ltFile) -Destination $ltStage
    }
    if (-not $SkipBootstrapper) { Copy-Item -LiteralPath $ltBootstrapper -Destination $ltStage }
    Copy-Item -LiteralPath (Join-Path $ltRoot 'notices/THIRD-PARTY-NOTICES.txt') -Destination $ltStage
    Copy-Item -LiteralPath (Join-Path $ltRoot 'docs/USER-README.md') -Destination (Join-Path $ltStage 'README.md')
    Copy-Item -LiteralPath (Join-Path $ltRoot 'LICENSE') -Destination $ltStage -ErrorAction SilentlyContinue

    # Smoke test: the staged sidecar must start from its own folder with only its own DLLs.
    $ltVersionOutput = & (Join-Path $ltStage 'llama-server.exe') --version 2>&1 | Out-String
    if ($LASTEXITCODE -ne 0 -or $ltVersionOutput -notmatch 'version') { throw "Staged llama-server failed its smoke test: $ltVersionOutput" }

    if (Test-Path -LiteralPath $ltZip) { Remove-Item -LiteralPath $ltZip -Force }
    Compress-Archive -Path $ltStage -DestinationPath $ltZip -CompressionLevel Optimal
    $ltHash = (Get-FileHash -LiteralPath $ltZip -Algorithm SHA256).Hash.ToLowerInvariant()
    Write-Host ("{0}  {1:N1} MB  sha256 {2}" -f $ltZip, ((Get-Item $ltZip).Length / 1MB), $ltHash)
    Write-Host 'Models are not included; the first-run screen downloads them.'
} finally {
    Pop-Location
}
