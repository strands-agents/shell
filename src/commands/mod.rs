mod basename;
mod cat;
mod chmod;
mod cp;
mod curl;
mod cut;
mod date;
mod dirname;
mod echo;
mod env;
mod r#false;
mod grep;
mod head;
mod jq;
mod ln;
mod ls;
mod mkdir;
mod mktemp;
mod mv;
mod pwd;
mod readlink;
mod rm;
mod rmdir;
mod sed;
mod sleep;
mod sort;
mod tail;
mod tee;
mod touch;
mod tr;
mod r#true;
mod uniq;
mod wc;

use std::future::Future;
use std::pin::Pin;

use crate::os::Kernel;

/// The result type for shell commands.
/// Ok(code) = clean exit, Err = abnormal termination.
pub type CommandResult = Result<i32, Box<dyn std::error::Error + Send + Sync>>;

/// The function signature for a shell command.
pub type CommandFn = for<'a> fn(
    &'a dyn Kernel,
    &'a [String],
) -> Pin<Box<dyn Future<Output = CommandResult> + Send + 'a>>;

/// A registered command entry.
pub struct CommandEntry {
    pub name: &'static str,
    pub func: CommandFn,
}

#[cfg(not(target_arch = "wasm32"))]
inventory::collect!(CommandEntry);

/// Look up a command by name.
pub fn lookup(name: &str) -> Option<CommandFn> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        for entry in inventory::iter::<CommandEntry> {
            if entry.name == name {
                return Some(entry.func);
            }
        }
        None
    }

    #[cfg(target_arch = "wasm32")]
    {
        // Static lookup table for WASM (inventory crate not available)
        let func: CommandFn = match name {
            "basename" => |os, args| Box::pin(basename::cmd_basename(os, args)),
            "cat" => |os, args| Box::pin(cat::cmd_cat(os, args)),
            "chmod" => |os, args| Box::pin(chmod::cmd_chmod(os, args)),
            "cp" => |os, args| Box::pin(cp::cmd_cp(os, args)),
            "curl" => |os, args| Box::pin(curl::cmd_curl(os, args)),
            "cut" => |os, args| Box::pin(cut::cmd_cut(os, args)),
            "date" => |os, args| Box::pin(date::cmd_date(os, args)),
            "dirname" => |os, args| Box::pin(dirname::cmd_dirname(os, args)),
            "echo" => |os, args| Box::pin(echo::cmd_echo(os, args)),
            "env" => |os, args| Box::pin(env::cmd_env(os, args)),
            "false" => |os, args| Box::pin(r#false::cmd_false(os, args)),
            "grep" => |os, args| Box::pin(grep::cmd_grep(os, args)),
            "head" => |os, args| Box::pin(head::cmd_head(os, args)),
            "jq" => |os, args| Box::pin(jq::cmd_jq(os, args)),
            "ln" => |os, args| Box::pin(ln::cmd_ln(os, args)),
            "ls" => |os, args| Box::pin(ls::cmd_ls(os, args)),
            "mkdir" => |os, args| Box::pin(mkdir::cmd_mkdir(os, args)),
            "mktemp" => |os, args| Box::pin(mktemp::cmd_mktemp(os, args)),
            "mv" => |os, args| Box::pin(mv::cmd_mv(os, args)),
            "pwd" => |os, args| Box::pin(pwd::cmd_pwd(os, args)),
            "readlink" => |os, args| Box::pin(readlink::cmd_readlink(os, args)),
            "rm" => |os, args| Box::pin(rm::cmd_rm(os, args)),
            "rmdir" => |os, args| Box::pin(rmdir::cmd_rmdir(os, args)),
            "sed" => |os, args| Box::pin(sed::cmd_sed(os, args)),
            "sleep" => |os, args| Box::pin(sleep::cmd_sleep(os, args)),
            "sort" => |os, args| Box::pin(sort::cmd_sort(os, args)),
            "tail" => |os, args| Box::pin(tail::cmd_tail(os, args)),
            "tee" => |os, args| Box::pin(tee::cmd_tee(os, args)),
            "touch" => |os, args| Box::pin(touch::cmd_touch(os, args)),
            "tr" => |os, args| Box::pin(tr::cmd_tr(os, args)),
            "true" => |os, args| Box::pin(r#true::cmd_true(os, args)),
            "uniq" => |os, args| Box::pin(uniq::cmd_uniq(os, args)),
            "wc" => |os, args| Box::pin(wc::cmd_wc(os, args)),
            _ => return None,
        };
        Some(func)
    }
}

/// Iterate over all registered commands.
#[cfg(not(target_arch = "wasm32"))]
pub fn iter() -> inventory::iter<CommandEntry> {
    inventory::iter::<CommandEntry>
}
