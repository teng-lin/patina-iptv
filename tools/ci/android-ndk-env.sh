# shellcheck shell=bash
# Source to compile C dependencies (TLS and SQLCipher) for Android: they need
# the NDK compiler and archive tools. Vendored OpenSSL uses ranlib separately;
# modern NDKs ship only llvm-ranlib.
export ANDROID_HOME=${ANDROID_HOME:-$HOME/Android/Sdk}
# The same version as .github/actions/android installs (test-android-changes.py).
IPTV_ANDROID_NDK=30.0.16248370
export ANDROID_NDK_HOME=${ANDROID_NDK_HOME:-$ANDROID_HOME/ndk/$IPTV_ANDROID_NDK}
toolchain="$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/linux-x86_64/bin"
export CC_x86_64_linux_android=${CC_x86_64_linux_android:-$toolchain/x86_64-linux-android26-clang}
export AR_x86_64_linux_android=${AR_x86_64_linux_android:-$toolchain/llvm-ar}
export RANLIB_x86_64_linux_android=${RANLIB_x86_64_linux_android:-$toolchain/llvm-ranlib}
export CC_aarch64_linux_android=${CC_aarch64_linux_android:-$toolchain/aarch64-linux-android26-clang}
export AR_aarch64_linux_android=${AR_aarch64_linux_android:-$toolchain/llvm-ar}
export RANLIB_aarch64_linux_android=${RANLIB_aarch64_linux_android:-$toolchain/llvm-ranlib}
