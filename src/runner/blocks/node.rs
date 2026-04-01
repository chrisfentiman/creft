use std::path::Path;

use crate::error::CreftError;
use crate::model::CodeBlock;

use super::BlockRunner;

pub(super) struct NodeRunner;

impl BlockRunner for NodeRunner {
    fn build_command(
        &self,
        block: &CodeBlock,
        script_path: &Path,
    ) -> Result<(std::process::Command, Option<tempfile::TempDir>), CreftError> {
        if !block.deps.is_empty() {
            let dir = tempfile::tempdir().map_err(CreftError::Io)?;
            // Write a stub package.json so npm installs into this directory.
            let pkg_json = dir.path().join("package.json");
            std::fs::write(&pkg_json, r#"{"private":true}"#).map_err(CreftError::Io)?;
            let status = std::process::Command::new("npm")
                .arg("install")
                .args(&block.deps)
                .current_dir(dir.path())
                .status()
                .map_err(|e| {
                    if e.kind() == std::io::ErrorKind::NotFound {
                        CreftError::InterpreterNotFound(
                            "npm (install Node.js). Run 'creft doctor' to check.".to_string(),
                        )
                    } else {
                        CreftError::Io(e)
                    }
                })?;
            if !status.success() {
                return Err(CreftError::Setup(format!(
                    "npm install failed for deps: {}",
                    block.deps.join(", ")
                )));
            }
            let node_modules = dir.path().join("node_modules");
            let mut c = std::process::Command::new("node");
            c.env("NODE_PATH", node_modules);
            c.arg(script_path);
            Ok((c, Some(dir)))
        } else {
            let mut c = std::process::Command::new("node");
            c.arg(script_path);
            Ok((c, None))
        }
    }
}
