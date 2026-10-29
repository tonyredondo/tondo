# Sourced by a step that builds for another architecture, aarch64 or
# x86_64, given as its argument: adds its targets, for glibc 2.28 and for
# musl, with zig as their C compiler and linker.
arch=$1
rustup target add $arch-unknown-linux-gnu $arch-unknown-linux-musl
for target in $arch-linux-gnu.2.28 $arch-linux-musl; do ln -sf "$PWD/dist/zig-cc" "$RUNNER_TEMP/$target-cc"; done
export CC_${arch}_unknown_linux_gnu=$RUNNER_TEMP/$arch-linux-gnu.2.28-cc
export CC_${arch}_unknown_linux_musl=$RUNNER_TEMP/$arch-linux-musl-cc
export AR_${arch}_unknown_linux_gnu="zig ar" AR_${arch}_unknown_linux_musl="zig ar"
export CARGO_TARGET_$(echo $arch | tr a-z A-Z)_UNKNOWN_LINUX_GNU_LINKER=$RUNNER_TEMP/$arch-linux-gnu.2.28-cc
