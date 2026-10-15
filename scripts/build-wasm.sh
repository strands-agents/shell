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
#   ./scripts/build-wasm.sh                      # debug build
#   ./scripts/build-wasm.sh --release            # release build
#   ./scripts/build-wasm.sh --reactor --release  # wasm32-wasip1 reactor (wasm-api feature)
#
set -euo pipefail

# --- Mode -----------------------------------------------------------------------
# Default: the wasm32-wasip2 command binary. --reactor: the wasm32-wasip1 cdylib
# exposing the shell_* C ABI (see src/wasm_api.rs) for embedding in WASI p1 hosts.
REACTOR=0
CARGO_ARGS=()
for arg in "$@"; do
    if [[ "$arg" == "--reactor" ]]; then
        REACTOR=1
    else
        CARGO_ARGS+=("$arg")
    fi
done
if [[ "$REACTOR" == 1 ]]; then
    TARGET="wasm32-wasip1"
    BUILD_ARGS=(--lib --features wasm-api)
    ARTIFACT="strands_shell.wasm"
else
    TARGET="wasm32-wasip2"
    BUILD_ARGS=(--features wasm --bin strands-shell-wasm)
    ARTIFACT="strands-shell-wasm.wasm"
fi

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
for t in wasm32_wasip1 wasm32_wasip2; do
    export "CC_$t=$WASI_SDK_PATH/bin/clang"
    export "AR_$t=$WASI_SDK_PATH/bin/ar"
    export "CFLAGS_$t=--sysroot=$WASI_SDK_PATH/share/wasi-sysroot"
done

# build.rs handles the linker search path for sysroot libraries
# (wasi-emulated-signal, setjmp) via cargo:rustc-link-search.
export WASI_SDK_PATH

# --- Build --------------------------------------------------------------------
cd "$(dirname "$0")/.."

cargo build --target "$TARGET" "${BUILD_ARGS[@]}" ${CARGO_ARGS[@]+"${CARGO_ARGS[@]}"}

PROFILE="debug"
for arg in ${CARGO_ARGS[@]+"${CARGO_ARGS[@]}"}; do
    if [[ "$arg" == "--release" ]]; then
        PROFILE="release"
    fi
done

WASM="target/$TARGET/$PROFILE/$ARTIFACT"
if [[ -f "$WASM" ]]; then
    echo ""
    echo "Built: $WASM ($(du -h "$WASM" | cut -f1))"
    echo ""
    if [[ "$REACTOR" == 1 ]]; then
        echo "Test with:"
        echo "  (cd tests/go && go test ./...)"
        echo "  (cd tests/java && mvn test)"
    else
        echo "Run with:"
        echo "  wasmtime -W exceptions=y -S http --dir /tmp $WASM"
    fi
fi
