//! Command-line entry point for the `strands-shell` binary.
//!
//! The logic lives in the library (rather than `main.rs`) so it can be reused
//! by the Python console-script entry point (`strands_shell._native:cli_main`),
//! letting `pip install strands-shell` / `uvx strands-shell` put the same CLI —
//! including the `--mcp` server — on the user's PATH from the wheel that also
//! ships the `_native` extension module.

use std::io::Write;

use clap::{Parser, Subcommand};
use rustyline::DefaultEditor;
use rustyline::error::ReadlineError;

use crate::Shell;

/// Strands Shell — A Virtual Shell for AI Agents
#[derive(Parser)]
#[command(name = "strands-shell", version, about)]
struct Cli {
    /// Path to a TOML config file (bind mounts, credentials)
    #[arg(long)]
    config: Option<String>,

    /// Execute a command string and exit
    #[arg(short = 'c')]
    command: Option<String>,

    /// Run as an MCP server over stdio
    #[arg(long)]
    mcp: bool,

    #[command(subcommand)]
    subcmd: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// List available built-in commands
    ListCommands,
}

fn build_shell(config: Option<&str>) -> Shell {
    let builder = Shell::builder();
    let builder = match config {
        Some(path) => match builder.config_file(path) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("strands-shell: --config: {e}");
                std::process::exit(1);
            }
        },
        None => builder,
    };
    match builder.build() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("strands-shell: {e}");
            std::process::exit(1);
        }
    }
}

/// Run the `strands-shell` CLI with the given argv (including the program name
/// as `args[0]`). Returns the process exit code. Interactive/`-c`/`--mcp` paths
/// call `std::process::exit` directly to match the standalone-binary behavior.
pub fn run<I, T>(args: I) -> i32
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
{
    let cli = Cli::parse_from(args);

    if let Some(subcmd) = &cli.subcmd {
        match subcmd {
            Commands::ListCommands => {
                let mut names: Vec<&str> = crate::commands::iter()
                    .into_iter()
                    .map(|c| c.name)
                    .collect();
                names.sort();
                for name in names {
                    println!("{name}");
                }
                return 0;
            }
        }
    }

    let mut shell = build_shell(cli.config.as_deref());

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("failed to create runtime");
    let local = tokio::task::LocalSet::new();

    if cli.mcp {
        rt.block_on(local.run_until(crate::mcp::serve(shell.kernel().clone(), shell.limits())));
        return 0;
    }

    // Start MCP servers if configured (must be inside the LocalSet)
    rt.block_on(local.run_until(shell.start_mcp()));

    if let Some(cmd) = &cli.command {
        let exit_code = rt.block_on(local.run_until(shell.execute(cmd)));
        let _ = std::io::stdout().flush();
        std::process::exit(exit_code);
    }

    let mut rl = DefaultEditor::new().expect("failed to initialize editor");

    loop {
        match rl.readline("$ ") {
            Ok(line) => {
                let line = line.trim().to_string();
                if line.is_empty() {
                    continue;
                }
                let _ = rl.add_history_entry(&line);

                let (exit_code, should_exit) = rt.block_on(local.run_until(async {
                    crate::exec::execute_with_reader(
                        shell.kernel().clone(),
                        &mut shell.proc,
                        &line,
                        &mut |_delim| rl.readline("> ").ok(),
                    )
                    .await
                }));
                let _ = std::io::stdout().flush();

                if should_exit {
                    std::process::exit(exit_code);
                }
            }
            Err(ReadlineError::Interrupted | ReadlineError::Eof) => break,
            Err(e) => {
                eprintln!("strands-shell: {e}");
                break;
            }
        }
    }

    0
}
