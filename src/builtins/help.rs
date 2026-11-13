use std::future::Future;
use std::pin::Pin;

use crate::commands::CommandResult;
use crate::os::{Kernel, Process};
use crate::prelude::*;

/// A help topic: the names it answers to, its synopsis, and its description.
struct Topic {
    names: &'static [&'static str],
    synopsis: &'static str,
    description: &'static str,
}

/// Every shell builtin, in the order `help` lists them.
const TOPICS: &[Topic] = &[
    Topic {
        names: &[":"],
        synopsis: ": [ARG...]",
        description: "Do nothing and exit 0.",
    },
    Topic {
        names: &["."],
        synopsis: ". FILE [ARG...]",
        description: "Run the commands in FILE in the current shell, with ARGs as the positional parameters.",
    },
    Topic {
        names: &["alias"],
        synopsis: "alias [NAME[=VALUE]...]",
        description: "Define an alias NAME for VALUE, or print the named aliases.\nWith no arguments, print every alias.",
    },
    Topic {
        names: &["break"],
        synopsis: "break [N]",
        description: "Leave the innermost for, while, or until loop, or N enclosing loops.",
    },
    Topic {
        names: &["builtin"],
        synopsis: "builtin NAME [ARG...]",
        description: "Run the builtin NAME with ARGs, bypassing any function of the same name.",
    },
    Topic {
        names: &["cd"],
        synopsis: "cd [DIR]",
        description: "Change the current directory to DIR, or to $HOME with no DIR.\n`cd -` changes to $OLDPWD.",
    },
    Topic {
        names: &["command"],
        synopsis: "command [-v | -V] NAME [ARG...]",
        description: "Run NAME with ARGs, bypassing any function of the same name.\n  -v  print how NAME would be found\n  -V  describe what NAME is",
    },
    Topic {
        names: &["continue"],
        synopsis: "continue [N]",
        description: "Start the next iteration of the innermost loop, or of the Nth enclosing loop.",
    },
    Topic {
        names: &["echo"],
        synopsis: "echo [-neE] [ARG...]",
        description: "Write the ARGs separated by spaces, then a newline.\n  -n  omit the trailing newline\n  -e  interpret backslash escapes (the default)\n  -E  do not interpret backslash escapes",
    },
    Topic {
        names: &["eval"],
        synopsis: "eval [ARG...]",
        description: "Join the ARGs with spaces and run the result as shell commands.",
    },
    Topic {
        names: &["exec"],
        synopsis: "exec [COMMAND [ARG...]]",
        description: "Run COMMAND, then exit the shell with its status.",
    },
    Topic {
        names: &["exit"],
        synopsis: "exit [N]",
        description: "Exit the shell with status N, or with the last command's status.",
    },
    Topic {
        names: &["export"],
        synopsis: "export NAME[=VALUE]...",
        description: "Mark each NAME for export to commands, optionally assigning VALUE.",
    },
    Topic {
        names: &["false"],
        synopsis: "false",
        description: "Do nothing and exit 1.",
    },
    Topic {
        names: &["find"],
        synopsis: "find [PATH...] [EXPRESSION]",
        description: "Search the directory trees under each PATH (default .) for files matching EXPRESSION.\nTests: -name, -iname, -path, -type, -size, -empty, -mtime, -mmin, -newer,\n-maxdepth, -mindepth. Operators: -not, -a, -o. Actions: -print, -print0, -exec.",
    },
    Topic {
        names: &["getopts"],
        synopsis: "getopts OPTSTRING NAME [ARG...]",
        description: "Parse the next option from the positional parameters (or ARGs) into NAME.\nA letter followed by `:` in OPTSTRING takes an argument, stored in $OPTARG.",
    },
    Topic {
        names: &["hash"],
        synopsis: "hash [-r] [NAME...]",
        description: "Remember the full path of each command NAME, or print the remembered paths.\n  -r  forget every remembered path",
    },
    Topic {
        names: &["help"],
        synopsis: "help [-s] [NAME...]",
        description: "Print usage for each builtin NAME, or list the builtins and commands.\nFor a command that is not a builtin, print `NAME --help`.\n  -s  print only the synopsis (the first line for a command)",
    },
    Topic {
        names: &["local"],
        synopsis: "local NAME[=VALUE]...",
        description: "Create a variable NAME visible only in the current function and the functions it calls.",
    },
    Topic {
        names: &["lua"],
        synopsis: "lua [-i] [-e CODE] [FILE [ARG...]]",
        description: "Run a Lua 5.4 script from FILE, CODE, or stdin.\n  -e CODE  run CODE\n  -i       start an interactive session",
    },
    Topic {
        names: &["printf"],
        synopsis: "printf FORMAT [ARG...]",
        description: "Write the ARGs formatted by FORMAT, reusing FORMAT until the ARGs run out.",
    },
    Topic {
        names: &["pwd"],
        synopsis: "pwd [-L | -P]",
        description: "Print the current directory.\n  -L  print $PWD (the default)\n  -P  print the path with symlinks resolved",
    },
    Topic {
        names: &["read"],
        synopsis: "read [-r] [-p PROMPT] [NAME...]",
        description: "Read a line from stdin and split it into the NAMEs (default REPLY).\n  -r         accepted for compatibility; backslashes are always kept\n  -p PROMPT  write PROMPT to stderr first",
    },
    Topic {
        names: &["readonly"],
        synopsis: "readonly NAME[=VALUE]...",
        description: "Mark each NAME as read-only, optionally assigning VALUE first.",
    },
    Topic {
        names: &["return"],
        synopsis: "return [N]",
        description: "Return from a function or sourced file with status N, or with the last command's status.",
    },
    Topic {
        names: &["set"],
        synopsis: "set [-eux] [-o NAME] [ARG...]  or  set -- [ARG...]",
        description: "Set shell options or the positional parameters. `+` instead of `-` turns an option off.\nWith no arguments, print every variable.\n  -e       exit when a command fails (errexit)\n  -u       treat unset variables as an error (nounset)\n  -x       print commands before running them (xtrace)\n  -o NAME  set option NAME, such as pipefail; with no NAME, list the options\n  --       assign the ARGs that follow to the positional parameters",
    },
    Topic {
        names: &["shift"],
        synopsis: "shift [N]",
        description: "Drop the first N positional parameters (default 1).",
    },
    Topic {
        names: &["shopt"],
        synopsis: "shopt [-s | -u] [-q] [-p] [OPTNAME...]",
        description: "Set or print bash-style shell options, as commands that can be reused as input.\n  -s  enable each OPTNAME\n  -u  disable each OPTNAME\n  -q  print nothing; the status tells whether each OPTNAME is enabled\n  -p  print the options (the default)",
    },
    Topic {
        names: &["source"],
        synopsis: "source FILE [ARG...]",
        description: "Run the commands in FILE in the current shell. The same as `.`.",
    },
    Topic {
        names: &["test", "["],
        synopsis: "test EXPR  or  [ EXPR ]",
        description: "Evaluate the conditional expression EXPR and exit 0 if it is true, 1 if false.\nFile tests such as -e, -f, -d; string tests such as -z, -n, =, !=;\ninteger tests such as -eq, -lt; and !, -a, -o to combine them.",
    },
    Topic {
        names: &["trap"],
        synopsis: "trap [ACTION SIGNAL...]",
        description: "Run ACTION when the shell exits. Only the EXIT signal is supported. `trap - EXIT` resets it.",
    },
    Topic {
        names: &["true"],
        synopsis: "true",
        description: "Do nothing and exit 0.",
    },
    Topic {
        names: &["type"],
        synopsis: "type NAME...",
        description: "Describe how each NAME would be run: alias, builtin, function, or command.",
    },
    Topic {
        names: &["umask"],
        synopsis: "umask [MODE]",
        description: "Set the file mode creation mask to the octal MODE, or print it.",
    },
    Topic {
        names: &["unalias"],
        synopsis: "unalias [-a] [NAME...]",
        description: "Remove each alias NAME.\n  -a  remove every alias",
    },
    Topic {
        names: &["unset"],
        synopsis: "unset [-f | -v] NAME...",
        description: "Remove each variable NAME.\n  -f  remove functions instead\n  -v  remove variables (the default)",
    },
    Topic {
        names: &["wait"],
        synopsis: "wait",
        description: "Wait for every background job to finish and exit with the last one's status.",
    },
    Topic {
        names: &["xargs"],
        synopsis: "xargs [-0] [-I STR] [-n N] [-d DELIM] [COMMAND [ARG...]]",
        description: "Run COMMAND (default echo) with ARGs plus the words read from stdin.\n  -0        split input on NUL\n  -d DELIM  split input on DELIM\n  -I STR    run COMMAND once per input word, replacing STR in ARGs with the word\n  -n N      pass at most N words per run",
    },
];

fn find_topic(name: &str) -> Option<&'static Topic> {
    TOPICS.iter().find(|t| t.names.contains(&name))
}

/// The names of the registered commands, sorted.
#[cfg(not(target_arch = "wasm32"))]
fn command_names() -> Vec<&'static str> {
    let mut names: Vec<&str> = crate::commands::iter()
        .into_iter()
        .map(|c| c.name)
        .filter(|name| find_topic(name).is_none())
        .collect();
    names.sort();
    names
}

/// Print usage for the named builtins, or list every builtin and command.
pub fn builtin_help<'a>(
    _os: &'a dyn Kernel,
    proc: &'a mut Process,
    args: &'a [String],
) -> Pin<Box<dyn Future<Output = CommandResult> + 'a>> {
    Box::pin(async move {
        let mut short = false;
        let mut names = args;
        while let Some(opt) = names.first().filter(|a| a.starts_with('-') && a.len() > 1) {
            match opt.as_str() {
                "-s" => short = true,
                "--" => {
                    names = &names[1..];
                    break;
                }
                _ => {
                    proc.err_msg(&format!("strands-shell: help: {opt}: invalid option"));
                    return Ok(2);
                }
            }
            names = &names[1..];
        }

        if names.is_empty() {
            let mut w = io::stdout()?;
            wprintln!(w, "Shell builtins (`help NAME` for details):")?;
            for topic in TOPICS {
                wprintln!(w, "  {}", topic.synopsis)?;
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                wprintln!(w)?;
                wprintln!(w, "Commands (`help NAME` or `NAME --help` for details):")?;
                wprintln!(w, "  {}", command_names().join(" "))?;
            }
            #[cfg(target_arch = "wasm32")]
            {
                wprintln!(w)?;
                wprintln!(w, "For a command, run `help NAME` or `NAME --help`.")?;
            }
            return Ok(0);
        }

        let mut w = io::stdout()?;
        let mut ew = io::stderr().ok();
        let mut status = 0;
        for name in names {
            if let Some(topic) = find_topic(name) {
                wprintln!(w, "{}: {}", name, topic.synopsis)?;
                if !short {
                    for line in topic.description.lines() {
                        wprintln!(w, "    {}", line)?;
                    }
                }
            } else if crate::commands::lookup(name).is_some() {
                // Run `NAME --help` without the shell's functions, aliases, or
                // tracing, so none of them can shadow or add to its output.
                let mut sub = proc.fork();
                sub.depth += 1;
                sub.capture = true;
                sub.opt_xtrace = false;
                sub.functions = Default::default();
                sub.aliases = Default::default();
                let line = format!("{} --help", super::xargs::shell_quote(name));
                let (exit, _) = Box::pin(crate::exec::execute(io::kernel(), &mut sub, &line)).await;
                let first_line = short
                    .then(|| sub.captured_output.lines().next())
                    .flatten()
                    .map(|line| format!("{line}\n"));
                let output = first_line.as_deref().unwrap_or(&sub.captured_output);
                w.write_all(output.as_bytes()).await?;
                if let Some(ew) = ew.as_mut() {
                    ew.write_all(sub.captured_stderr.as_bytes()).await?;
                }
                if exit != 0 {
                    status = exit;
                }
            } else {
                proc.err_msg(&format!(
                    "strands-shell: help: no help topics match '{name}'"
                ));
                status = 1;
            }
        }
        Ok(status)
    })
}
