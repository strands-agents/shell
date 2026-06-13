use crate::prelude::*;

#[command("false")]
async fn cmd_false(_os: &dyn Kernel, _args: &[String]) -> CommandResult {
    Ok(1)
}
