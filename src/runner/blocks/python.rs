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

#[cfg(test)]
mod tests {
    use std::path::Path;

    use pretty_assertions::assert_eq;

    use crate::model::CodeBlock;

    use super::*;

    fn make_block(deps: Vec<&str>) -> CodeBlock {
        CodeBlock {
            lang: "python".to_string(),
            code: String::new(),
            deps: deps.into_iter().map(str::to_string).collect(),
            llm_config: None,
            llm_parse_error: None,
        }
    }

    #[test]
    fn build_command_no_deps_uses_python3() {
        let block = make_block(vec![]);
        let script = Path::new("/tmp/script.py");
        let (cmd, dir) = PythonRunner.build_command(&block, script).unwrap();
        assert_eq!(cmd.get_program(), "python3");
        let args: Vec<_> = cmd.get_args().collect();
        assert_eq!(args, ["/tmp/script.py"]);
        assert!(dir.is_none());
    }

    #[test]
    fn build_command_with_deps_uses_uv_run_with() {
        let block = make_block(vec!["requests", "httpx"]);
        let script = Path::new("/tmp/script.py");
        let (cmd, dir) = PythonRunner.build_command(&block, script).unwrap();
        assert_eq!(cmd.get_program(), "uv");
        let args: Vec<String> = cmd
            .get_args()
            .map(|a| a.to_str().unwrap().to_string())
            .collect();
        assert_eq!(
            args,
            [
                "run",
                "--with",
                "requests",
                "--with",
                "httpx",
                "--",
                "python3",
                "/tmp/script.py"
            ]
        );
        assert!(dir.is_none());
    }

    #[test]
    fn build_command_with_single_dep_uses_uv_run_with() {
        let block = make_block(vec!["numpy"]);
        let script = Path::new("/tmp/script.py");
        let (cmd, _) = PythonRunner.build_command(&block, script).unwrap();
        assert_eq!(cmd.get_program(), "uv");
        let args: Vec<String> = cmd
            .get_args()
            .map(|a| a.to_str().unwrap().to_string())
            .collect();
        assert_eq!(
            args,
            ["run", "--with", "numpy", "--", "python3", "/tmp/script.py"]
        );
    }
}
