mod alias;
mod cd;
mod colon;
mod echo;
mod export;
mod find;
mod getopts;
mod hash;
mod local;
pub mod lua;
mod printf;
mod pwd;
mod read;
mod set;
mod shift;
mod test;
mod trap;
mod type_cmd;
mod umask;
mod unset;
mod wait;
mod xargs;

use crate::commands::CommandResult;
use crate::os::{Kernel, Process};

/// Builtin function signature: operates directly on the shell process.
pub type BuiltinFn =
    for<'a> fn(
        &'a dyn Kernel,
        &'a mut Process,
        &'a [String],
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = CommandResult> + 'a>>;

/// Look up a builtin by name.
pub fn lookup(name: &str) -> Option<BuiltinFn> {
    match name {
        ":" | "true" => Some(colon::builtin_colon),
        "alias" => Some(alias::builtin_alias),
        "cd" => Some(cd::builtin_cd),
        "echo" => Some(echo::builtin_echo),
        "export" => Some(export::builtin_export),
        "false" => Some(colon::builtin_false),
        "find" => Some(find::builtin_find),
        "getopts" => Some(getopts::builtin_getopts),
        "hash" => Some(hash::builtin_hash),
        "local" => Some(local::builtin_local),
        "lua" => Some(lua::builtin_lua),
        "printf" => Some(printf::builtin_printf),
        "pwd" => Some(pwd::builtin_pwd),
        "read" => Some(read::builtin_read),
        "readonly" => Some(export::builtin_readonly),
        "set" => Some(set::builtin_set),
        "shift" => Some(shift::builtin_shift),
        "test" | "[" => Some(test::builtin_test),
        "trap" => Some(trap::builtin_trap),
        "type" => Some(type_cmd::builtin_type),
        "umask" => Some(umask::builtin_umask),
        "unalias" => Some(alias::builtin_unalias),
        "unset" => Some(unset::builtin_unset),
        "wait" => Some(wait::builtin_wait),
        "xargs" => Some(xargs::builtin_xargs),
        _ => None,
    }
}
