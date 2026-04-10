use std::path::Path;

use crate::error::CreftError;
use crate::model::{CodeBlock, LlmConfig};

use super::BlockRunner;

/// Runner for LLM provider CLI commands.
///
/// Builds a provider CLI invocation (e.g. `claude -p`) from the block's
/// `llm_config`. The prompt is delivered via stdin by the caller —
/// `_script_path` is a dummy file that exists but is ignored.
pub(super) struct LlmRunner;

impl BlockRunner for LlmRunner {
    fn build_command(
        &self,
        block: &CodeBlock,
        _script_path: &Path,
    ) -> Result<(std::process::Command, Option<tempfile::TempDir>), CreftError> {
        let config = block.llm_config.as_ref().ok_or_else(|| {
            CreftError::Setup("llm block is missing provider configuration".into())
        })?;
        Ok((build_llm_command(config), None))
    }
}

/// Build a `Command` for the given LLM provider.
///
/// Does NOT configure stdin/stdout/stderr or cwd — the caller does that.
/// Does NOT set env vars — the caller does that.
///
/// Provider patterns:
/// - `claude`: `claude -p [--model <model>]`
/// - `gemini`: `gemini -p [-m <model>]`
/// - `codex`: `codex exec -`
/// - `ollama`: `ollama run [<model>]`
/// - unknown: `<provider> [--model <model>]`
///
/// `params` is split on whitespace and appended as individual arguments.
pub(crate) fn build_llm_command(config: &LlmConfig) -> std::process::Command {
    let provider = if config.provider.is_empty() {
        "claude"
    } else {
        &config.provider
    };

    let mut cmd = match provider {
        "claude" => {
            let mut c = std::process::Command::new("claude");
            c.arg("-p");
            if !config.model.is_empty() {
                c.arg("--model").arg(&config.model);
            }
            c
        }
        "gemini" => {
            let mut c = std::process::Command::new("gemini");
            c.arg("-p");
            if !config.model.is_empty() {
                c.arg("-m").arg(&config.model);
            }
            c
        }
        "codex" => {
            let mut c = std::process::Command::new("codex");
            c.arg("exec").arg("-");
            // codex does not take a model flag in exec mode
            c
        }
        "ollama" => {
            let mut c = std::process::Command::new("ollama");
            c.arg("run");
            if !config.model.is_empty() {
                c.arg(&config.model);
            }
            c
        }
        unknown => {
            let mut c = std::process::Command::new(unknown);
            if !config.model.is_empty() {
                c.arg("--model").arg(&config.model);
            }
            c
        }
    };

    if !config.params.is_empty() {
        for token in config.params.split_whitespace() {
            cmd.arg(token);
        }
    }

    cmd
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use pretty_assertions::assert_eq;

    fn make_config(provider: &str, model: &str, params: &str) -> LlmConfig {
        LlmConfig {
            provider: provider.to_string(),
            model: model.to_string(),
            params: params.to_string(),
        }
    }

    #[test]
    fn build_llm_command_claude_default() {
        let config = make_config("", "", "");
        let cmd = build_llm_command(&config);
        let prog = format!("{:?}", cmd.get_program());
        assert_eq!(prog, "\"claude\"");
        let args: Vec<_> = cmd.get_args().collect();
        assert_eq!(args, ["-p"]);
    }

    #[test]
    fn build_llm_command_claude_with_model() {
        let config = make_config("claude", "claude-opus-4-5", "");
        let cmd = build_llm_command(&config);
        let args: Vec<_> = cmd.get_args().collect();
        assert_eq!(args, ["-p", "--model", "claude-opus-4-5"]);
    }

    #[test]
    fn build_llm_command_gemini_with_model() {
        let config = make_config("gemini", "gemini-pro", "");
        let cmd = build_llm_command(&config);
        let prog = format!("{:?}", cmd.get_program());
        assert_eq!(prog, "\"gemini\"");
        let args: Vec<_> = cmd.get_args().collect();
        assert_eq!(args, ["-p", "-m", "gemini-pro"]);
    }

    #[test]
    fn build_llm_command_codex() {
        let config = make_config("codex", "", "");
        let cmd = build_llm_command(&config);
        let prog = format!("{:?}", cmd.get_program());
        assert_eq!(prog, "\"codex\"");
        let args: Vec<_> = cmd.get_args().collect();
        assert_eq!(args, ["exec", "-"]);
    }

    #[test]
    fn build_llm_command_ollama_with_model() {
        let config = make_config("ollama", "llama3", "");
        let cmd = build_llm_command(&config);
        let prog = format!("{:?}", cmd.get_program());
        assert_eq!(prog, "\"ollama\"");
        let args: Vec<_> = cmd.get_args().collect();
        assert_eq!(args, ["run", "llama3"]);
    }

    #[test]
    fn build_llm_command_unknown_provider() {
        let config = make_config("myprovider", "mymodel", "");
        let cmd = build_llm_command(&config);
        let prog = format!("{:?}", cmd.get_program());
        assert_eq!(prog, "\"myprovider\"");
        let args: Vec<_> = cmd.get_args().collect();
        assert_eq!(args, ["--model", "mymodel"]);
    }

    #[test]
    fn build_llm_command_params_split_on_whitespace() {
        let config = make_config("claude", "", "--verbose --output json");
        let cmd = build_llm_command(&config);
        let args: Vec<_> = cmd.get_args().collect();
        assert_eq!(args, ["-p", "--verbose", "--output", "json"]);
    }

    #[test]
    fn build_command_returns_error_when_llm_config_absent() {
        let block = CodeBlock {
            lang: "llm".to_string(),
            code: "say hello".to_string(),
            deps: vec![],
            llm_config: None,
            llm_parse_error: None,
        };
        let result = LlmRunner.build_command(&block, Path::new("/tmp/dummy"));
        match result {
            Err(CreftError::Setup(msg)) => {
                assert!(
                    msg.contains("missing provider configuration"),
                    "expected error message to mention 'missing provider configuration', got: {msg}"
                );
            }
            other => panic!("expected Err(CreftError::Setup(...)), got {other:?}"),
        }
    }
}
