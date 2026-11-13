use md5::Md5;

use super::checksum::{self, Algorithm};
use crate::prelude::*;

const MD5: Algorithm = Algorithm {
    command: "md5sum",
    tag: "MD5",
    new_hasher: || Box::new(Md5::default()),
};

#[command("md5sum")]
async fn cmd_md5sum(os: &dyn Kernel, args: &[String]) -> CommandResult {
    checksum::run(os, args, &MD5).await
}
