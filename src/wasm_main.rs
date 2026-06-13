//! WASM entry point for Strands Shell.
//!
//! Reads shell commands from WASI stdin and executes them, writing output
//! to WASI stdout/stderr. Each instance runs in an isolated WASM linear
//! memory, making it safe to spawn many instances per machine.
//!
//! ## Usage
//!
//! Wasmtime requires `-W exceptions=y` (for Lua's setjmp/longjmp error handling)
//! and `-S http` (for curl / `wasi:http`).
//!
//! ```bash
//! # Simple script
//! echo 'echo hello' | wasmtime -W exceptions=y -S http strands-shell-wasm.wasm
//!
//! # Lua script
//! echo 'lua -e "print(math.sqrt(144))"' | wasmtime -W exceptions=y -S http strands-shell-wasm.wasm
//!
//! # Mount a host directory into the VFS (copies files into memory)
//! echo 'ls /workspace' | wasmtime -W exceptions=y -S http \
//!     --dir /path/to/project \
//!     strands-shell-wasm.wasm -- --mount /path/to/project:/workspace
//!
//! # Multiple mounts
//! echo 'cat /data/input.txt | grep error > /workspace/results.txt' | wasmtime \
//!     -W exceptions=y -S http \
//!     --dir /tmp/data --dir /home/user/src \
//!     strands-shell-wasm.wasm -- --mount /tmp/data:/data --mount /home/user/src:/workspace
//!
//! # Set environment variables
//! echo 'echo $MY_VAR' | wasmtime -W exceptions=y -S http \
//!     strands-shell-wasm.wasm -- --env MY_VAR=hello
//! ```
//!
//! **Note:** The `--dir` flag is required by Wasmtime to grant WASI access to
//! host directories. The `--mount` flag tells Strands Shell to copy those files into
//! its in-memory VFS at the given virtual path.

use std::io::Read;

use strands_shell::Shell;

fn main() {
    let args: Vec<String> = std::env::args().collect();

    let mut mounts: Vec<(String, String)> = Vec::new();
    let mut envs: Vec<(String, String)> = Vec::new();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--mount" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("strands-shell-wasm: --mount requires SOURCE:DEST argument");
                    std::process::exit(1);
                }
                let parts: Vec<&str> = args[i].splitn(2, ':').collect();
                if parts.len() != 2 {
                    eprintln!(
                        "strands-shell-wasm: --mount format is SOURCE:DEST (got '{}')",
                        args[i]
                    );
                    std::process::exit(1);
                }
                mounts.push((parts[0].to_string(), parts[1].to_string()));
            }
            "--env" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("strands-shell-wasm: --env requires KEY=VALUE argument");
                    std::process::exit(1);
                }
                let parts: Vec<&str> = args[i].splitn(2, '=').collect();
                if parts.len() != 2 {
                    eprintln!(
                        "strands-shell-wasm: --env format is KEY=VALUE (got '{}')",
                        args[i]
                    );
                    std::process::exit(1);
                }
                envs.push((parts[0].to_string(), parts[1].to_string()));
            }
            "--" => {} // skip separator (wasmtime passes this)
            other => {
                eprintln!("strands-shell-wasm: unknown argument '{other}'");
                eprintln!(
                    "Usage: strands-shell-wasm [--mount SOURCE:DEST]... [--env KEY=VALUE]..."
                );
                std::process::exit(1);
            }
        }
        i += 1;
    }

    // Read the entire script from WASI stdin
    let mut script = String::new();
    std::io::stdin()
        .read_to_string(&mut script)
        .unwrap_or_else(|e| {
            eprintln!("strands-shell-wasm: failed to read stdin: {e}");
            std::process::exit(1);
        });

    if script.trim().is_empty() {
        return;
    }

    // Build a shell, copying any mounted directories into the in-memory VFS
    let mut builder = Shell::builder();
    for (source, dest) in &mounts {
        builder = builder.bind(source, dest);
    }
    for (key, value) in &envs {
        builder = builder.env(key, value);
    }

    let mut shell = builder.build().unwrap_or_else(|e| {
        eprintln!("strands-shell-wasm: failed to build shell: {e}");
        std::process::exit(1);
    });

    // Execute the script using tokio's single-threaded runtime
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap_or_else(|e| {
            eprintln!("strands-shell-wasm: failed to create runtime: {e}");
            std::process::exit(1);
        });

    let local = tokio::task::LocalSet::new();
    let exit_code = rt.block_on(local.run_until(async {
        let output = shell.run(&script).await;
        if !output.stdout.is_empty() {
            print!("{}", output.stdout);
        }
        if !output.stderr.is_empty() {
            eprint!("{}", output.stderr);
        }
        output.status
    }));

    std::process::exit(exit_code);
}
