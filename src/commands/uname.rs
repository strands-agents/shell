use crate::prelude::*;

const HELP: &str = "Usage: uname [OPTION]...
Print information about the shell's system. With no OPTION, same as -s.
  -a, --all                print all information, in the order below,
                           except -p and -i
  -s, --kernel-name        print the kernel name
  -n, --nodename           print the host name
  -r, --kernel-release     print the kernel release
  -v, --kernel-version     print the kernel version
  -m, --machine            print the machine hardware name
  -p, --processor          print the processor type (always unknown)
  -i, --hardware-platform  print the hardware platform (always unknown)
  -o, --operating-system   print the operating system";

const SYSNAME: u8 = 1;
const NODENAME: u8 = 1 << 1;
const RELEASE: u8 = 1 << 2;
const VERSION: u8 = 1 << 3;
const MACHINE: u8 = 1 << 4;
const OS: u8 = 1 << 5;
const PROCESSOR: u8 = 1 << 6;
const PLATFORM: u8 = 1 << 7;

#[command("uname")]
async fn cmd_uname(os: &dyn Kernel, args: &[String]) -> CommandResult {
    let mut parser = lexopt::Parser::from_args(args);
    let mut fields = 0;
    let mut all = false;
    while let Some(arg) = parser.next()? {
        match arg {
            Long("help") => {
                let mut w = io::stdout()?;
                wprintln!(w, "{}", HELP)?;
                return Ok(0);
            }
            Short('a') | Long("all") => all = true,
            Short('s') | Long("kernel-name") => fields |= SYSNAME,
            Short('n') | Long("nodename") => fields |= NODENAME,
            Short('r') | Long("kernel-release") => fields |= RELEASE,
            Short('v') | Long("kernel-version") => fields |= VERSION,
            Short('m') | Long("machine") => fields |= MACHINE,
            Short('o') | Long("operating-system") => fields |= OS,
            Short('p') | Long("processor") => fields |= PROCESSOR,
            Short('i') | Long("hardware-platform") => fields |= PLATFORM,
            _ => return Err(arg.unexpected().into()),
        }
    }
    if all {
        // Like GNU, -a leaves out the unknown processor and platform.
        fields = !(PROCESSOR | PLATFORM);
    } else if fields == 0 {
        fields = SYSNAME;
    }
    let system = os.uname();
    let host = io::with_process(|p| match p.get_env("HOSTNAME") {
        Some(host) if !host.is_empty() => host.to_string(),
        _ => system.node_name.clone(),
    });
    let values = [
        (SYSNAME, system.kernel_name.as_str()),
        (NODENAME, host.as_str()),
        (RELEASE, system.kernel_release.as_str()),
        (VERSION, system.kernel_version.as_str()),
        (MACHINE, system.machine.as_str()),
        (PROCESSOR, "unknown"),
        (PLATFORM, "unknown"),
        (OS, system.operating_system.as_str()),
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
