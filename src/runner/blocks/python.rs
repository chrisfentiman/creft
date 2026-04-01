use std::path::Path;

use crate::error::CreftError;
use crate::model::CodeBlock;

use super::BlockRunner;

pub(super) struct PythonRunner;

impl BlockRunner for PythonRunner {
    fn build_command(
        &self,
        block: &CodeBlock,
        script_path: &Path,
    ) -> Result<(std::process::Command, Option<tempfile::TempDir>), CreftError> {
        if !block.deps.is_empty() {
            let mut c = std::process::Command::new("uv");
            c.arg("run");
            for dep in &block.deps {
                c.arg("--with").arg(dep);
            }
            c.arg("--").arg("python3").arg(script_path);
            Ok((c, None))
        } else {
            let mut c = std::process::Command::new("python3");
            c.arg(script_path);
            Ok((c, None))
        }
    }
}
