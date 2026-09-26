# Script de build Android pour SilenceLess.
#
# Configure les variables d'environnement (SDK/NDK + compilateur croisé) puis
# lance `cargo tauri android build`.
#
# Usage :
#   .\build-android.ps1                     # toutes les ABIs (aarch64, armv7, i686, x86_64)
#   .\build-android.ps1 -Target aarch64     # une seule ABI
#   .\build-android.ps1 -Debug              # build de débogage (plus rapide)

param(
    [string]$Target = "",
    [switch]$Debug
)

$ErrorActionPreference = "Stop"

$sdk = "$env:LOCALAPPDATA\Android\Sdk"
$ndk = "$sdk\ndk\27.0.12077973"
$bin = "$ndk\toolchains\llvm\prebuilt\windows-x86_64\bin"

if (-not (Test-Path $bin)) {
    throw "NDK introuvable dans $bin. Installe le SDK Android et le NDK 27 (via Android Studio ou sdkmanager)."
}

$env:ANDROID_HOME = $sdk
$env:ANDROID_NDK_HOME = $ndk
$env:NDK_HOME = $ndk

# Compilateur C croisé + éditeur de liens Rust pour chaque ABI.
# Le crate `cc` et Rust ne détectent pas le NDK seuls : on les pointe dessus.
$env:CC_aarch64_linux_android = "$bin\aarch64-linux-android21-clang.cmd"
$env:AR_aarch64_linux_android = "$bin\llvm-ar.exe"
$env:CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER = "$bin\aarch64-linux-android21-clang.cmd"

$env:CC_armv7_linux_androideabi = "$bin\armv7a-linux-androideabi21-clang.cmd"
$env:AR_armv7_linux_androideabi = "$bin\llvm-ar.exe"
$env:CARGO_TARGET_ARMV7_LINUX_ANDROIDEABI_LINKER = "$bin\armv7a-linux-androideabi21-clang.cmd"

$env:CC_i686_linux_android = "$bin\i686-linux-android21-clang.cmd"
$env:AR_i686_linux_android = "$bin\llvm-ar.exe"
$env:CARGO_TARGET_I686_LINUX_ANDROID_LINKER = "$bin\i686-linux-android21-clang.cmd"

$env:CC_x86_64_linux_android = "$bin\x86_64-linux-android21-clang.cmd"
$env:AR_x86_64_linux_android = "$bin\llvm-ar.exe"
$env:CARGO_TARGET_X86_64_LINUX_ANDROID_LINKER = "$bin\x86_64-linux-android21-clang.cmd"

$env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path"

$args = @()
if ($Debug) { $args += "--debug" }
if ($Target) { $args += "--target"; $args += $Target }

& cargo tauri android build @args
