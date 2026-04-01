use std::path::Path;

use crate::error::CreftError;
use crate::model::CodeBlock;

use super::BlockRunner;

pub(super) struct RubyRunner;

impl BlockRunner for RubyRunner {
    fn build_command(
        &self,
        block: &CodeBlock,
        script_path: &Path,
    ) -> Result<(std::process::Command, Option<tempfile::TempDir>), CreftError> {
        let _ = block; // deps not supported for ruby; lang is already dispatched
        let mut c = std::process::Command::new("ruby");
        c.arg(script_path);
        Ok((c, None))
    }
}
