#!/usr/bin/env bash
#
# Build strands-shell for the wasm32-wasip2 target.
#
# Prerequisites:
#   - Rust with the wasm32-wasip2 target:  rustup target add wasm32-wasip2
#   - wasi-sdk >= 32:  https://github.com/WebAssembly/wasi-sdk/releases
#
# The script finds wasi-sdk from (in order):
#   1. WASI_SDK_PATH environment variable
#   2. /opt/wasi-sdk
#   3. ~/wasi-sdk
#
# Usage:
#   ./scripts/build-wasm.sh            # debug build
#   ./scripts/build-wasm.sh --release  # release build
#
set -euo pipefail

# --- Locate wasi-sdk ----------------------------------------------------------
if [[ -z "${WASI_SDK_PATH:-}" ]]; then
    for candidate in /opt/wasi-sdk "$HOME/wasi-sdk"; do
        if [[ -d "$candidate/share/wasi-sysroot" ]]; then
            WASI_SDK_PATH="$candidate"
            break
        fi
    done
fi

if [[ -z "${WASI_SDK_PATH:-}" ]] || [[ ! -d "$WASI_SDK_PATH/share/wasi-sysroot" ]]; then
    echo >&2 "ERROR: wasi-sdk not found."
    echo >&2 ""
    echo >&2 "Install wasi-sdk >= 32 and either:"
    echo >&2 "  • export WASI_SDK_PATH=/path/to/wasi-sdk"
    echo >&2 "  • place it at /opt/wasi-sdk or ~/wasi-sdk"
    echo >&2 ""
    echo >&2 "Download: https://github.com/WebAssembly/wasi-sdk/releases"
    exit 1
fi

echo "Using wasi-sdk at: $WASI_SDK_PATH"
echo "  clang: $("$WASI_SDK_PATH/bin/clang" --version | head -1)"

# --- Configure the C toolchain for the cc crate ------------------------------
# The cc crate (used by lua-src to compile Lua's C sources) reads these env vars
# with target-specific suffixes (hyphens replaced by underscores).
export CC_wasm32_wasip2="$WASI_SDK_PATH/bin/clang"
export AR_wasm32_wasip2="$WASI_SDK_PATH/bin/ar"
export CFLAGS_wasm32_wasip2="--sysroot=$WASI_SDK_PATH/share/wasi-sysroot"

# build.rs handles the linker search path for sysroot libraries
# (wasi-emulated-signal, setjmp) via cargo:rustc-link-search.
export WASI_SDK_PATH

# --- Build --------------------------------------------------------------------
cd "$(dirname "$0")/.."

cargo build \
    --target wasm32-wasip2 \
    --features wasm \
    --bin strands-shell-wasm \
    "$@"

PROFILE="debug"
for arg in "$@"; do
    if [[ "$arg" == "--release" ]]; then
        PROFILE="release"
    fi
done

WASM="target/wasm32-wasip2/$PROFILE/strands-shell-wasm.wasm"
if [[ -f "$WASM" ]]; then
    echo ""
    echo "Built: $WASM ($(du -h "$WASM" | cut -f1))"
    echo ""
    echo "Run with:"
    echo "  wasmtime -W exceptions=y -S http --dir /tmp $WASM"
fi
