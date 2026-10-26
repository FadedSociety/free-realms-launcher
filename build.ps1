param(
    [ValidateSet('build', 'test')][string]$Action = 'build',
    [string]$LlvmMingw = $env:LLVM_MINGW_ROOT,
    [switch]$Release
)
$ErrorActionPreference = 'Stop'
# The launcher links with an llvm-mingw UCRT toolchain (https://github.com/mstorsjo/llvm-mingw) for the
# x86_64-pc-windows-gnullvm target: `rustup target add x86_64-pc-windows-gnullvm` once.
$linker = if ($LlvmMingw) { Join-Path $LlvmMingw 'bin\x86_64-w64-mingw32-clang.exe' }
if (!$linker -or !(Test-Path -LiteralPath $linker -PathType Leaf)) {
    throw 'Set LLVM_MINGW_ROOT or -LlvmMingw to an unpacked llvm-mingw UCRT toolchain.'
}
# Do not add llvm-mingw/bin to PATH: host build scripts must keep their host compiler.
$env:CARGO_TARGET_X86_64_PC_WINDOWS_GNULLVM_LINKER = [IO.Path]::GetFullPath($linker)
$env:CARGO_TARGET_X86_64_PC_WINDOWS_GNULLVM_RUSTFLAGS = '-C target-feature=+crt-static'
if (!$env:CARGO_TARGET_DIR) {
    $env:CARGO_TARGET_DIR = Join-Path $PSScriptRoot 'target'
}
$cargo = 'cargo'
if ($env:CARGO_HOME -and (Test-Path -LiteralPath (Join-Path $env:CARGO_HOME 'bin\cargo.exe'))) {
    $cargo = Join-Path $env:CARGO_HOME 'bin\cargo.exe'
}
$cargoArgs = @($Action, '--locked', '--target', 'x86_64-pc-windows-gnullvm',
    '--manifest-path', (Join-Path $PSScriptRoot 'Cargo.toml'))
if ($Release) { $cargoArgs += '--release' }
& $cargo @cargoArgs
exit $LASTEXITCODE
