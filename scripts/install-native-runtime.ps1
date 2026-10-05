# Native test executables need DLLs in their own directory, including deps/.
[CmdletBinding()]
param([string]$LibraryDir = '', [string]$TargetDir = '')
$ErrorActionPreference = 'Stop'
$ltRoot = Split-Path -Parent $PSScriptRoot
if (-not $LibraryDir) {
    $ltDeps = if ($env:LT_DEPS_DIR) { $env:LT_DEPS_DIR } else { Join-Path $ltRoot '.deps' }
    $LibraryDir = Join-Path $ltDeps 'native/sherpa-1.13.8-no-tts-ort-1.30.0/lib'
}
if (-not $TargetDir) { $TargetDir = Join-Path $ltRoot 'target' }
$ltResolvedRoot = [IO.Path]::GetFullPath($ltRoot) + [IO.Path]::DirectorySeparatorChar
$ltResolvedTarget = [IO.Path]::GetFullPath($TargetDir)
if (-not $ltResolvedTarget.StartsWith($ltResolvedRoot, [StringComparison]::OrdinalIgnoreCase)) {
    throw 'Native test outputs must remain inside the workspace.'
}
if (-not (Test-Path -LiteralPath (Join-Path $LibraryDir 'onnxruntime.dll'))) {
    throw 'Run scripts/setup.ps1 to prepare the pinned native runtime first.'
}
$ltProfiles = @(
    'debug', 'release',
    'x86_64-pc-windows-msvc/debug', 'x86_64-pc-windows-msvc/release'
)
foreach ($ltProfile in $ltProfiles) {
    $ltProfileDir = Join-Path $ltResolvedTarget $ltProfile
    if (-not (Test-Path -LiteralPath $ltProfileDir -PathType Container)) { continue }
    foreach ($ltSubDir in @('', 'deps', 'examples')) {
        $ltOutput = if ($ltSubDir) { Join-Path $ltProfileDir $ltSubDir } else { $ltProfileDir }
        New-Item -ItemType Directory -Path $ltOutput -Force | Out-Null
        foreach ($ltDll in (Get-ChildItem -LiteralPath $LibraryDir -Filter '*.dll' -File)) {
            Copy-Item -LiteralPath $ltDll.FullName -Destination $ltOutput -Force
        }
    }
}
