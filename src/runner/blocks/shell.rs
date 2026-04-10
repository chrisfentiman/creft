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

#[cfg(test)]
mod tests {
    use std::path::Path;

    use pretty_assertions::assert_eq;
    use rstest::rstest;

    use crate::model::CodeBlock;

    use super::*;

    fn make_block(lang: &str) -> CodeBlock {
        CodeBlock {
            lang: lang.to_string(),
            code: String::new(),
            deps: vec![],
            llm_config: None,
            llm_parse_error: None,
        }
    }

    #[test]
    fn build_command_bash_program_is_bash() {
        let block = make_block("bash");
        let script = Path::new("/tmp/script.sh");
        let (cmd, dir) = ShellRunner.build_command(&block, script).unwrap();
        assert_eq!(cmd.get_program(), "bash");
        assert!(dir.is_none());
    }

    #[rstest]
    #[case::typescript("typescript", "npx", "tsx")]
    #[case::ts("ts", "npx", "tsx")]
    fn build_command_multi_token_interpreter_splits_into_program_and_arg(
        #[case] lang: &str,
        #[case] expected_program: &str,
        #[case] expected_first_arg: &str,
    ) {
        let block = make_block(lang);
        let script = Path::new("/tmp/script.ts");
        let (cmd, _) = ShellRunner.build_command(&block, script).unwrap();
        assert_eq!(cmd.get_program(), expected_program);
        let args: Vec<_> = cmd.get_args().collect();
        assert_eq!(
            args[0].to_str().unwrap(),
            expected_first_arg,
            "first arg of multi-token interpreter must be second token"
        );
        // Last arg must be the script path.
        assert_eq!(args.last().unwrap().to_str().unwrap(), "/tmp/script.ts");
    }

    #[test]
    fn build_command_appends_script_path_as_last_arg() {
        let block = make_block("bash");
        let script = Path::new("/tmp/myscript.sh");
        let (cmd, _) = ShellRunner.build_command(&block, script).unwrap();
        let args: Vec<_> = cmd.get_args().collect();
        assert_eq!(args.last().unwrap().to_str().unwrap(), "/tmp/myscript.sh");
    }
}
