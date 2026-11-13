use crate::prelude::*;

const HELP: &str = "Usage: uname [OPTION]...
Print information about the shell's system. With no OPTION, same as -s.
  -a, --all                print all information, in the order below
  -s, --kernel-name        print the kernel name
  -n, --nodename           print the host name
  -r, --kernel-release     print the kernel release
  -v, --kernel-version     print the kernel version
  -m, --machine            print the machine hardware name
  -o, --operating-system   print the operating system";

const KERNEL_NAME: &str = "Linux";
/// The release is the Strands Shell version, so it never shows the host's kernel.
const KERNEL_RELEASE: &str = env!("CARGO_PKG_VERSION");
const KERNEL_VERSION: &str = "#1";
const OPERATING_SYSTEM: &str = "Linux";
/// The host name when `$HOSTNAME` is unset or empty, matching `hostname`.
const DEFAULT_HOSTNAME: &str = "localhost";

const SYSNAME: u8 = 1;
const NODENAME: u8 = 1 << 1;
const RELEASE: u8 = 1 << 2;
const VERSION: u8 = 1 << 3;
const MACHINE: u8 = 1 << 4;
const OS: u8 = 1 << 5;

#[command("uname")]
async fn cmd_uname(_os: &dyn Kernel, args: &[String]) -> CommandResult {
    let mut parser = lexopt::Parser::from_args(args);
    let mut fields = 0;
    while let Some(arg) = parser.next()? {
        match arg {
            Long("help") => {
                let mut w = io::stdout()?;
                wprintln!(w, "{}", HELP)?;
                return Ok(0);
            }
            Short('a') | Long("all") => {
                fields |= SYSNAME | NODENAME | RELEASE | VERSION | MACHINE | OS
            }
            Short('s') | Long("kernel-name") => fields |= SYSNAME,
            Short('n') | Long("nodename") => fields |= NODENAME,
            Short('r') | Long("kernel-release") => fields |= RELEASE,
            Short('v') | Long("kernel-version") => fields |= VERSION,
            Short('m') | Long("machine") => fields |= MACHINE,
            Short('o') | Long("operating-system") => fields |= OS,
            _ => return Err(arg.unexpected().into()),
        }
    }
    if fields == 0 {
        fields = SYSNAME;
    }
    let host = io::with_process(|p| match p.get_env("HOSTNAME") {
        Some(host) if !host.is_empty() => host.to_string(),
        _ => DEFAULT_HOSTNAME.to_string(),
    });
    let values = [
        (SYSNAME, KERNEL_NAME),
        (NODENAME, host.as_str()),
        (RELEASE, KERNEL_RELEASE),
        (VERSION, KERNEL_VERSION),
        (MACHINE, std::env::consts::ARCH),
        (OS, OPERATING_SYSTEM),
    ];
    let line: Vec<&str> = values
        .iter()
        .filter(|(bit, _)| fields & bit != 0)
        .map(|(_, value)| *value)
        .collect();
    let mut w = io::stdout()?;
    wprintln!(w, "{}", line.join(" "))?;
    Ok(0)
}
