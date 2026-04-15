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
            let output = std::process::Command::new("npm")
                .arg("install")
                .args(&block.deps)
                .current_dir(dir.path())
                .output()
                .map_err(|e| {
                    if e.kind() == std::io::ErrorKind::NotFound {
                        CreftError::InterpreterNotFound(
                            "npm (install Node.js). Run 'creft doctor' to check.".to_string(),
                        )
                    } else {
                        CreftError::Io(e)
                    }
                })?;
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(CreftError::Setup(format!(
                    "npm install failed for deps: {}\n{}",
                    block.deps.join(", "),
                    stderr.trim(),
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

#[cfg(test)]
mod tests {
    use std::path::Path;

    use pretty_assertions::assert_eq;

    use crate::model::CodeBlock;

    use super::super::BlockRunner;
    use super::NodeRunner;

    fn make_node_block(deps: Vec<&str>) -> CodeBlock {
        CodeBlock {
            lang: "node".to_string(),
            code: String::new(),
            deps: deps.into_iter().map(str::to_owned).collect(),
            llm_config: None,
            llm_parse_error: None,
        }
    }

    /// The no-deps path produces a bare `node <script>` command with no TempDir
    /// and no NODE_PATH override. This is the contract the deps path must not
    /// disturb for blocks that carry no npm dependencies.
    #[test]
    fn build_command_without_deps_returns_bare_node_command() {
        let runner = NodeRunner;
        let block = make_node_block(vec![]);
        let script = Path::new("/tmp/test_script.js");
        let (cmd, dir) = runner.build_command(&block, script).unwrap();

        assert_eq!(cmd.get_program().to_str().unwrap(), "node");
        assert!(dir.is_none(), "no-deps path must not allocate a TempDir");

        let args: Vec<_> = cmd.get_args().collect();
        assert_eq!(args, [script.as_os_str()]);

        // No NODE_PATH override — the system node_modules resolution is used.
        let node_path: Vec<_> = cmd.get_envs().filter(|(k, _)| *k == "NODE_PATH").collect();
        assert!(node_path.is_empty(), "no-deps path must not set NODE_PATH");
    }
}
