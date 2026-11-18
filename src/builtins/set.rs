use std::future::Future;
use std::pin::Pin;

use crate::commands::CommandResult;
use crate::os::{Kernel, Process};
use crate::prelude::*;

pub fn builtin_set<'a>(
    _os: &'a dyn Kernel,
    proc: &'a mut Process,
    args: &'a [String],
) -> Pin<Box<dyn Future<Output = CommandResult> + 'a>> {
    Box::pin(async move {
        if args.is_empty() {
            // Print all variables
            let mut w = io::stdout()?;
            let mut vars: Vec<_> = proc.env.iter().collect();
            vars.sort_by_key(|(k, _)| (*k).clone());
            for (k, v) in vars {
                wprintln!(w, "{}={}", k, v)?;
            }
            return Ok(0);
        }

        // `set --` clears positional params; `set -- a b c` sets them
        if args[0] == "--" {
            proc.args = args[1..].to_vec();
            return Ok(0);
        }

        let mut listings = Vec::new();
        let mut positional = None;
        let mut i = 0;
        while i < args.len() {
            let a = &args[i];
            i += 1;
            if !(a.starts_with('-') || a.starts_with('+')) {
                // Positional params
                positional = Some(args[i - 1..].to_vec());
                break;
            }
            let enable = a.starts_with('-');
            for ch in a[1..].chars() {
                let opt = match ch {
                    'e' => Opt::Errexit,
                    'u' => Opt::Nounset,
                    'x' => Opt::Xtrace,
                    // `-o NAME` takes the next word, even inside a cluster like `-euo`.
                    'o' => {
                        let Some(name) = args.get(i) else {
                            listings.push(enable);
                            continue;
                        };
                        i += 1;
                        let Some(opt) = Opt::from_name(name) else {
                            proc.err_msg(&format!(
                                "strands-shell: set: {name}: invalid option name"
                            ));
                            return Ok(2);
                        };
                        opt
                    }
                    _ => {
                        proc.err_msg(&format!("strands-shell: set: -{ch}: unsupported option"));
                        return Ok(2);
                    }
                };
                *opt.flag(proc) = enable;
            }
        }

        // A bare `-o` lists the options; a bare `+o` lists them as `set` commands.
        for enable in listings {
            let mut w = io::stdout()?;
            for opt in Opt::ALL {
                let on = *opt.flag(proc);
                if enable {
                    wprintln!(w, "{:<15}\t{}", opt.name(), if on { "on" } else { "off" })?;
                } else {
                    wprintln!(w, "set {}o {}", if on { '-' } else { '+' }, opt.name())?;
                }
            }
        }
        if let Some(positional) = positional {
            proc.args = positional;
        }

        Ok(0)
    })
}

/// A shell option that `set` can change.
#[derive(Clone, Copy)]
enum Opt {
    Errexit,
    Nounset,
    Pipefail,
    Xtrace,
}

impl Opt {
    const ALL: [Opt; 4] = [Opt::Errexit, Opt::Nounset, Opt::Pipefail, Opt::Xtrace];

    fn name(self) -> &'static str {
        match self {
            Opt::Errexit => "errexit",
            Opt::Nounset => "nounset",
            Opt::Pipefail => "pipefail",
            Opt::Xtrace => "xtrace",
        }
    }

    fn from_name(name: &str) -> Option<Opt> {
        Opt::ALL.into_iter().find(|opt| opt.name() == name)
    }

    fn flag(self, proc: &mut Process) -> &mut bool {
        match self {
            Opt::Errexit => &mut proc.opt_errexit,
            Opt::Nounset => &mut proc.opt_nounset,
            Opt::Pipefail => &mut proc.opt_pipefail,
            Opt::Xtrace => &mut proc.opt_xtrace,
        }
    }
}
