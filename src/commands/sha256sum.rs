use sha2::Sha256;

use super::checksum::{self, Algorithm};
use crate::prelude::*;

const SHA256: Algorithm = Algorithm {
    command: "sha256sum",
    tag: "SHA256",
    new_hasher: || Box::new(Sha256::default()),
};

#[command("sha256sum")]
async fn cmd_sha256sum(os: &dyn Kernel, args: &[String]) -> CommandResult {
    checksum::run(os, args, &SHA256).await
}
