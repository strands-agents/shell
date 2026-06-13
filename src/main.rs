fn main() {
    let exit_code = strands_shell::cli::run(std::env::args_os());
    std::process::exit(exit_code);
}
