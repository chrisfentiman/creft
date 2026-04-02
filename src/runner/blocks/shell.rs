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
        // Split on whitespace to handle multi-token interpreters like "npx tsx".
        // interpreter() may return "npx tsx" for TypeScript blocks; constructing
        // Command::new("npx tsx") would fail with NotFound since the binary name
        // includes a space. Splitting gives Command::new("npx") with arg "tsx".
        let parts: Vec<&str> = interp.split_whitespace().collect();
        let mut c = std::process::Command::new(parts[0]);
        for part in &parts[1..] {
            c.arg(part);
        }
        c.arg(script_path);
        Ok((c, None))
    }
}
