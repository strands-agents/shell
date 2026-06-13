use crate::prelude::*;

#[command("true")]
async fn cmd_true(_os: &dyn Kernel, _args: &[String]) -> CommandResult {
    Ok(0)
}
