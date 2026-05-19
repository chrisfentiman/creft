use std::path::Path;

use crate::error::CreftError;
use crate::model::CodeBlock;

use super::super::interpreter;
use super::{BlockRunner, warn_missing_deps};

/// Generic file-mode interpreter runner.
///
/// Builds a `<interpreter> [flags...] <script_path>` command. Used for:
///   - shell family (`bash`/`sh`/`zsh`) — the original role, name preserved
///   - typescript family (`typescript`/`ts`) — `npx tsx` is multi-token
///     but still file mode
///   - unknown tags whose author opted into file mode via `# extension:`
///
/// Family-specific runners (`PythonRunner`, `NodeRunner`, `LlmRunner`) exist
/// because their command shapes differ structurally — `uv run` for python
/// deps, `npx --package` chains for node deps, provider CLI dispatch for
/// LLM. Anything that boils down to "spawn an interpreter on a file" lives
/// here.
pub(super) struct ShellRunner;

impl BlockRunner for ShellRunner {
    fn build_command(
        &self,
        block: &CodeBlock,
        script_path: &Path,
        flags: &[String],
    ) -> Result<(std::process::Command, Option<tempfile::TempDir>), CreftError> {
        warn_missing_deps(block);

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
        // Flags are placed between the interpreter and the script path.
        for flag in flags {
            c.arg(flag);
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
            extension: None,
            flags: None,
            llm_config: None,
            llm_parse_error: None,
        }
    }

    fn make_block_with_flags(lang: &str, flags_str: &str) -> CodeBlock {
        CodeBlock {
            lang: lang.to_string(),
            code: String::new(),
            deps: vec![],
            extension: None,
            flags: Some(flags_str.to_string()),
            llm_config: None,
            llm_parse_error: None,
        }
    }

    #[test]
    fn build_command_bash_program_is_bash() {
        let block = make_block("bash");
        let script = Path::new("/tmp/script.sh");
        let (cmd, dir) = ShellRunner.build_command(&block, script, &[]).unwrap();
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
        let (cmd, _) = ShellRunner.build_command(&block, script, &[]).unwrap();
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
        let (cmd, _) = ShellRunner.build_command(&block, script, &[]).unwrap();
        let args: Vec<_> = cmd.get_args().collect();
        assert_eq!(args.last().unwrap().to_str().unwrap(), "/tmp/myscript.sh");
    }

    /// Flags are inserted between the interpreter and the script path.
    #[test]
    fn build_command_bash_with_flags_inserts_before_script_path() {
        let block = make_block_with_flags("bash", "-x");
        let script = Path::new("/tmp/myscript.sh");
        let flags = vec!["-x".to_string()];
        let (cmd, _) = ShellRunner.build_command(&block, script, &flags).unwrap();
        let args: Vec<_> = cmd.get_args().map(|a| a.to_str().unwrap()).collect();
        assert_eq!(args, ["-x", "/tmp/myscript.sh"]);
    }

    /// Unknown tag with extension goes through ShellRunner; interpreter resolves to the tag.
    #[test]
    fn build_command_unknown_tag_with_extension_uses_tag_as_binary() {
        let block = CodeBlock {
            lang: "ruby".to_string(),
            code: String::new(),
            deps: vec![],
            extension: Some("rb".to_string()),
            flags: None,
            llm_config: None,
            llm_parse_error: None,
        };
        let script = Path::new("/tmp/creft-XXXXX.rb");
        let flags = vec!["-e".to_string()];
        let (cmd, _) = ShellRunner.build_command(&block, script, &flags).unwrap();
        assert_eq!(cmd.get_program(), "ruby");
        let args: Vec<_> = cmd.get_args().map(|a| a.to_str().unwrap()).collect();
        assert_eq!(args, ["-e", "/tmp/creft-XXXXX.rb"]);
    }
}
