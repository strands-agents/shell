use crate::prelude::*;
use crate::vfs::{LASH_GID, LASH_UID};

const HELP: &str = "Usage: id [OPTION]...
Print the user and group IDs of the shell's user.
  -u, --user    print only the user ID
  -g, --group   print only the group ID
  -n, --name    with -u or -g, print a name instead of a number";

/// The user name when `$USER` is unset or empty. The group shares the name.
const DEFAULT_USER: &str = "lash";

#[derive(Clone, Copy, PartialEq)]
enum Only {
    All,
    User,
    Group,
}

#[command("id")]
async fn cmd_id(_os: &dyn Kernel, args: &[String]) -> CommandResult {
    let mut parser = lexopt::Parser::from_args(args);
    let mut only = Only::All;
    let mut name = false;
    while let Some(arg) = parser.next()? {
        let choice = match arg {
            Long("help") => {
                let mut w = io::stdout()?;
                wprintln!(w, "{}", HELP)?;
                return Ok(0);
            }
            Short('u') | Long("user") => Only::User,
            Short('g') | Long("group") => Only::Group,
            Short('n') | Long("name") => {
                name = true;
                continue;
            }
            _ => return Err(arg.unexpected().into()),
        };
        if only != Only::All && only != choice {
            return Err("cannot print \"only\" of more than one choice".into());
        }
        only = choice;
    }
    let user = io::with_process(|p| match p.get_env("USER") {
        Some(user) if !user.is_empty() => user.to_string(),
        _ => DEFAULT_USER.to_string(),
    });
    let line = match (only, name) {
        (Only::All, true) => {
            return Err("cannot print only names or real IDs in default format".into());
        }
        (Only::All, false) => {
            format!("uid={LASH_UID}({user}) gid={LASH_GID}({user}) groups={LASH_GID}({user})")
        }
        (Only::User, false) => LASH_UID.to_string(),
        (Only::Group, false) => LASH_GID.to_string(),
        (_, true) => user,
    };
    let mut w = io::stdout()?;
    wprintln!(w, "{}", line)?;
    Ok(0)
}
