//! Build script for strands-shell.
//!
//! When cross-compiling for a WASI target this script adds the wasi-sdk
//! sysroot library directory to the linker search path so it can find
//! `-lwasi-emulated-signal` and `-lsetjmp` (required by Lua's C sources).
//!
//! The C compiler / archiver env vars (`CC_wasm32_wasip2`, etc.) must be
//! set *before* invoking Cargo because each crate's build script runs in
//! its own process.  Use `scripts/build-wasm.sh` which handles this.
//!
//! On native targets this script is a no-op.

use std::env;
use std::path::PathBuf;

fn main() {
    let target = env::var("TARGET").unwrap_or_default();

    // napi-build wires up the symbol export setup the napi runtime needs.
    // The `node` feature pulls in napi-build as an optional build-dependency.
    #[cfg(feature = "node")]
    napi_build::setup();

    if !target.contains("wasi") {
        return;
    }

    let sdk = resolve_wasi_sdk();
    let sysroot = sdk.join("share/wasi-sysroot");

    // --- Linker search path for WASI sysroot libraries ------------------------
    // Lua's WASI build links against libwasi-emulated-signal and libsetjmp which
    // live in the wasi-sdk sysroot.
    let lib_dir = sysroot.join("lib").join(&target);
    if lib_dir.exists() {
        println!("cargo:rustc-link-search=native={}", lib_dir.display());
    } else {
        // Fall back to the base wasm32-wasi lib dir
        let fallback = sysroot.join("lib/wasm32-wasi");
        if fallback.exists() {
            println!("cargo:rustc-link-search=native={}", fallback.display());
        }
    }

    println!("cargo:rerun-if-env-changed=WASI_SDK_PATH");
}

/// Find wasi-sdk from WASI_SDK_PATH, /opt/wasi-sdk, or ~/wasi-sdk.
fn resolve_wasi_sdk() -> PathBuf {
    if let Ok(p) = env::var("WASI_SDK_PATH") {
        let sdk = PathBuf::from(p);
        assert!(
            sdk.join("share/wasi-sysroot").exists(),
            "WASI_SDK_PATH={} does not contain share/wasi-sysroot",
            sdk.display()
        );
        return sdk;
    }

    let candidates = [
        PathBuf::from("/opt/wasi-sdk"),
        env::var("HOME")
            .map(|h| PathBuf::from(h).join("wasi-sdk"))
            .unwrap_or_default(),
    ];

    for c in &candidates {
        if c.join("share/wasi-sysroot").exists() {
            return c.clone();
        }
    }

    panic!(
        "\n\
        ========================================================================\n\
        wasi-sdk not found.  Building for WASI requires wasi-sdk >= 32.\n\
        \n\
        Install it and either:\n\
          • Set WASI_SDK_PATH to the install directory, or\n\
          • Place it at /opt/wasi-sdk or ~/wasi-sdk\n\
        \n\
        Then use scripts/build-wasm.sh which sets the C toolchain env vars.\n\
        \n\
        Download: https://github.com/WebAssembly/wasi-sdk/releases\n\
        ========================================================================"
    );
}
