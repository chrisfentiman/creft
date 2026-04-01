use std::path::Path;

use crate::error::CreftError;
use crate::model::CodeBlock;

use super::super::{interpreter, which};
use super::BlockRunner;

pub(super) struct ShellRunner;

impl BlockRunner for ShellRunner {
    fn build_command(
        &self,
        block: &CodeBlock,
        script_path: &Path,
    ) -> Result<(std::process::Command, Option<tempfile::TempDir>), CreftError> {
        if !block.deps.is_empty() {
            for dep in &block.deps {
                if which(dep).is_none() {
                    eprintln!("warning: '{}' not found on PATH", dep);
                }
            }
        }
        let interp = interpreter(&block.lang);
        let mut c = std::process::Command::new(interp);
        c.arg(script_path);
        Ok((c, None))
    }
}
