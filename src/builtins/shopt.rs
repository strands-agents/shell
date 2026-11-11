use std::future::Future;
use std::pin::Pin;

use crate::commands::CommandResult;
use crate::os::{Kernel, Process};
use crate::prelude::*;

/// The option names bash's `shopt` knows.
const OPTIONS: &[&str] = &[
    "autocd",
    "assoc_expand_once",
    "cdable_vars",
    "cdspell",
    "checkhash",
    "checkjobs",
    "checkwinsize",
    "cmdhist",
    "compat31",
    "compat32",
    "compat40",
    "compat41",
    "compat42",
    "compat43",
    "compat44",
    "complete_fullquote",
    "direxpand",
    "dirspell",
    "dotglob",
    "execfail",
    "expand_aliases",
    "extdebug",
    "extglob",
    "extquote",
    "failglob",
    "force_fignore",
    "globasciiranges",
    "globskipdots",
    "globstar",
    "gnu_errfmt",
    "histappend",
    "histreedit",
    "histverify",
    "hostcomplete",
    "huponexit",
    "inherit_errexit",
    "interactive_comments",
    "lastpipe",
    "lithist",
    "localvar_inherit",
    "localvar_unset",
    "login_shell",
    "mailwarn",
    "no_empty_cmd_completion",
    "nocaseglob",
    "nocasematch",
    "noexpand_translation",
    "nullglob",
    "patsub_replacement",
    "progcomp",
    "progcomp_alias",
    "promptvars",
    "restricted_shell",
    "shift_verbose",
    "sourcepath",
    "varredir_close",
    "xpg_echo",
];

/// The options `shopt -s` may turn on, because the shell already behaves as they say.
const HONOURED: &[&str] = &["expand_aliases"];

/// `shopt [-pqsu] [name …]`: the option state is recorded and has no effect on execution.
pub fn builtin_shopt<'a>(
    _os: &'a dyn Kernel,
    proc: &'a mut Process,
    args: &'a [String],
) -> Pin<Box<dyn Future<Output = CommandResult> + 'a>> {
    Box::pin(async move {
        let mut set = false;
        let mut unset = false;
        let mut quiet = false;
        let mut names: Vec<&str> = Vec::new();
        for arg in args {
            match arg.as_str() {
                "-s" => set = true,
                "-u" => unset = true,
                "-q" => quiet = true,
                "-p" => {}
                "-o" => {
                    proc.err_msg("strands-shell: shopt: -o: unsupported option");
                    return Ok(2);
                }
                flag if flag.starts_with('-') && flag.len() > 1 => {
                    proc.err_msg(&format!("strands-shell: shopt: {flag}: invalid option"));
                    return Ok(2);
                }
                name => names.push(name),
            }
        }
        if set && unset {
            proc.err_msg("strands-shell: shopt: cannot set and unset shell options simultaneously");
            return Ok(1);
        }
        let mut status = 0;
        for name in &names {
            if !OPTIONS.contains(name) {
                proc.err_msg(&format!(
                    "strands-shell: shopt: {name}: invalid shell option name"
                ));
                status = 1;
            }
        }
        if status != 0 {
            return Ok(status);
        }
        if set && let Some(name) = names.iter().find(|name| !HONOURED.contains(name)) {
            proc.err_msg(&format!("strands-shell: shopt: {name}: not supported"));
            return Ok(1);
        }
        if set || unset {
            for name in names {
                if set {
                    proc.shopts.insert(name.to_string());
                } else {
                    proc.shopts.remove(name);
                }
            }
            return Ok(0);
        }
        let listed: Vec<&str> = if names.is_empty() {
            OPTIONS.to_vec()
        } else {
            names
        };
        let mut w = io::stdout()?;
        for name in listed {
            let on = proc.shopts.contains(name);
            if !on {
                status = 1;
            }
            if !quiet {
                wprintln!(w, "shopt -{} {}", if on { 's' } else { 'u' }, name)?;
            }
        }
        Ok(status)
    })
}
