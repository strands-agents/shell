use std::future::Future;
use std::pin::Pin;

use crate::commands::CommandResult;
use crate::os::{Kernel, Process};

pub fn builtin_getopts<'a>(
    _os: &'a dyn Kernel,
    proc: &'a mut Process,
    args: &'a [String],
) -> Pin<Box<dyn Future<Output = CommandResult> + 'a>> {
    Box::pin(async move {
        if args.len() < 2 {
            proc.err_msg("strands-shell: getopts: usage: getopts optstring var [arg ...]");
            return Ok(2);
        }

        let optstr = &args[0];
        let varname = &args[1];
        let silent = optstr.starts_with(':');

        // Arguments to parse: explicit args or positional params
        let optargs: Vec<String> = if args.len() > 2 {
            args[2..].to_vec()
        } else {
            proc.args.clone()
        };

        // Read OPTIND from env (1-based index into optargs)
        let mut ind: usize = proc
            .env
            .get("OPTIND")
            .and_then(|s| s.parse().ok())
            .unwrap_or(1);
        let mut off = proc.optoff;

        // Reset if OPTIND was set to 1
        if ind <= 1 {
            ind = 1;
            off = -1;
        }

        // Get current position within the argument
        let arg_idx = ind - 1; // 0-based
        let p = if off >= 0 {
            optargs.get(arg_idx.wrapping_sub(1)).and_then(|a| {
                if (off as usize) < a.len() {
                    Some(&a[off as usize..])
                } else {
                    None
                }
            })
        } else {
            None
        };

        // If no more chars in current arg, advance to next
        let (c, rest, next_ind) = if let Some(p) = p.filter(|s| !s.is_empty()) {
            let mut chars = p.chars();
            let c = chars.next().unwrap();
            let rest = chars.as_str();
            (c, rest.to_string(), ind)
        } else {
            // Advance to next arg starting with '-'
            let a = match optargs.get(arg_idx) {
                Some(a) if a.starts_with('-') && a.len() > 1 && a != "--" => a,
                Some(a) if a == "--" => {
                    // -- terminates options; OPTIND points past it
                    proc.set_env(varname, "?");
                    proc.set_env("OPTIND", (arg_idx + 2).to_string());
                    proc.optoff = -1;
                    return Ok(1);
                }
                _ => {
                    // Done
                    proc.set_env(varname, "?");
                    proc.set_env("OPTIND", (arg_idx + 1).to_string());
                    proc.optoff = -1;
                    return Ok(1);
                }
            };
            let mut chars = a[1..].chars(); // skip leading '-'
            let c = chars.next().unwrap();
            let rest = chars.as_str();
            (c, rest.to_string(), ind + 1)
        };

        // Look up option in optstr
        let spec = optstr.trim_start_matches(':');
        let mut found = false;
        let mut takes_arg = false;
        let mut si = spec.chars().peekable();
        while let Some(sc) = si.next() {
            if sc == c {
                found = true;
                takes_arg = si.peek() == Some(&':');
                break;
            }
            if si.peek() == Some(&':') {
                si.next();
            }
        }

        if !found {
            // Unknown option
            if silent {
                proc.set_env("OPTARG", c.to_string());
            } else {
                proc.err_msg(&format!("strands-shell: getopts: illegal option -- {c}"));
                proc.unset_env("OPTARG");
            }
            proc.set_env(varname, "?");
            // Update position
            if rest.is_empty() {
                proc.optoff = -1;
                proc.set_env("OPTIND", next_ind.to_string());
            } else {
                let prev_arg = optargs.get(next_ind - 2).map(|a| a.as_str()).unwrap_or("");
                proc.optoff = (prev_arg.len() - rest.len()) as i32;
                proc.set_env("OPTIND", next_ind.to_string());
            }
            return Ok(0);
        }

        if takes_arg {
            // Option requires argument
            if !rest.is_empty() {
                // Rest of current arg is the argument
                proc.set_env("OPTARG", &rest);
                proc.optoff = -1;
                proc.set_env("OPTIND", next_ind.to_string());
            } else {
                // Next arg is the argument
                let arg_val = optargs.get(next_ind - 1);
                match arg_val {
                    Some(v) => {
                        proc.set_env("OPTARG", v);
                        proc.optoff = -1;
                        proc.set_env("OPTIND", (next_ind + 1).to_string());
                    }
                    None => {
                        // Missing argument
                        if silent {
                            proc.set_env("OPTARG", c.to_string());
                            proc.set_env(varname, ":");
                        } else {
                            proc.err_msg(&format!(
                                "strands-shell: getopts: option requires an argument -- {c}"
                            ));
                            proc.unset_env("OPTARG");
                            proc.set_env(varname, "?");
                        }
                        proc.optoff = -1;
                        proc.set_env("OPTIND", next_ind.to_string());
                        return Ok(0);
                    }
                }
            }
        } else {
            proc.set_env("OPTARG", "");
            if rest.is_empty() {
                proc.optoff = -1;
                proc.set_env("OPTIND", next_ind.to_string());
            } else {
                let prev_arg = optargs.get(next_ind - 2).map(|a| a.as_str()).unwrap_or("");
                proc.optoff = (prev_arg.len() - rest.len()) as i32;
                proc.set_env("OPTIND", next_ind.to_string());
            }
        }

        proc.set_env(varname, c.to_string());
        Ok(0)
    })
}
