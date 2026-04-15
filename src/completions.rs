//! Shell completion script generation.
//!
//! Completion scripts call `creft list --names` at completion time to pick up
//! the current skill registry without requiring regeneration.

use crate::error::CreftError;

/// Generate a shell completion script to stdout.
///
/// The generated script completes built-in command names statically
/// and calls `creft list --names` at completion time for dynamic skill names.
///
/// Supported shells: `"bash"`, `"zsh"`, `"fish"`.
/// Any other value returns [`CreftError::CliParse`].
pub(crate) fn generate(shell: &str) -> Result<String, CreftError> {
    match shell {
        "bash" => Ok(bash_script()),
        "zsh" => Ok(zsh_script()),
        "fish" => Ok(fish_script()),
        other => Err(CreftError::CliParse(format!(
            "unsupported shell: {other}\n\nSupported shells: bash, zsh, fish\n\nUsage: creft completions <shell>"
        ))),
    }
}

fn bash_script() -> String {
    r#"_creft() {
    local cur="${COMP_WORDS[COMP_CWORD]}"

    if [ "$COMP_CWORD" -eq 1 ]; then
        local builtins="add list show remove plugin settings up init doctor completions help"
        local skills
        skills="$(creft list --names 2>/dev/null)"
        COMPREPLY=($(compgen -W "$builtins $skills" -- "$cur"))
        return
    fi

    case "${COMP_WORDS[1]}" in
        plugin)
            COMPREPLY=($(compgen -W "install update uninstall activate deactivate list search" -- "$cur"))
            ;;
        settings)
            COMPREPLY=($(compgen -W "show set" -- "$cur"))
            ;;
        completions)
            COMPREPLY=($(compgen -W "bash zsh fish" -- "$cur"))
            ;;
        *)
            COMPREPLY=()
            ;;
    esac
}
complete -F _creft creft
"#
    .to_string()
}

fn zsh_script() -> String {
    r#"#compdef creft

_creft() {
    local state

    _arguments \
        '1: :->command' \
        '*: :->args'

    case "$state" in
        command)
            local -a builtins skills
            builtins=(
                'add:Save a skill from stdin'
                'completions:Generate shell completions'
                'doctor:Check environment and skill health'
                'init:Initialize local skill storage'
                'list:List available skills'
                'plugin:Manage skill collections'
                'remove:Delete a skill'
                'settings:Manage settings'
                'show:Show a skill'\''s full definition'
                'up:Install creft for your coding AI'
            )
            local skill_names
            skill_names=(${(f)"$(creft list --names 2>/dev/null)"})
            for name in $skill_names; do
                skills+=("$name")
            done
            _describe 'command' builtins
            _describe 'skill' skills
            ;;
        args)
            case "${words[2]}" in
                plugin)
                    local -a subcommands
                    subcommands=(
                        'install:Install a plugin from a git repository'
                        'update:Update installed plugins'
                        'uninstall:Remove an installed plugin'
                        'activate:Make plugin commands available in a scope'
                        'deactivate:Remove plugin commands from a scope'
                        'list:List installed plugins'
                        'search:Search for commands across installed plugins'
                    )
                    _describe 'subcommand' subcommands
                    ;;
                settings)
                    local -a subcommands
                    subcommands=(
                        'show:Show current settings'
                        'set:Set a configuration value'
                    )
                    _describe 'subcommand' subcommands
                    ;;
                completions)
                    local -a shells
                    shells=('bash:Bash (4.0+)' 'zsh:Zsh (5.0+)' 'fish:Fish (3.0+)')
                    _describe 'shell' shells
                    ;;
            esac
            ;;
    esac
}

_creft "$@"
"#
    .to_string()
}

fn fish_script() -> String {
    r#"# creft fish completions

# Disable file completions for creft
complete -c creft -f

# Built-in commands
complete -c creft -n '__fish_use_subcommand' -a 'add'         -d 'Save a skill from stdin'
complete -c creft -n '__fish_use_subcommand' -a 'completions' -d 'Generate shell completions'
complete -c creft -n '__fish_use_subcommand' -a 'doctor'      -d 'Check environment and skill health'
complete -c creft -n '__fish_use_subcommand' -a 'init'        -d 'Initialize local skill storage'
complete -c creft -n '__fish_use_subcommand' -a 'list'        -d 'List available skills'
complete -c creft -n '__fish_use_subcommand' -a 'plugin'      -d 'Manage skill collections'
complete -c creft -n '__fish_use_subcommand' -a 'remove'      -d 'Delete a skill'
complete -c creft -n '__fish_use_subcommand' -a 'settings'    -d 'Manage settings'
complete -c creft -n '__fish_use_subcommand' -a 'show'        -d "Show a skill's full definition"
complete -c creft -n '__fish_use_subcommand' -a 'up'          -d 'Install creft for your coding AI'

# Dynamic skill names from registry
complete -c creft -n '__fish_use_subcommand' -a '(creft list --names 2>/dev/null)'

# plugin subcommands
complete -c creft -n '__fish_seen_subcommand_from plugin' -a 'install'    -d 'Install a plugin from a git repository'
complete -c creft -n '__fish_seen_subcommand_from plugin' -a 'update'     -d 'Update installed plugins'
complete -c creft -n '__fish_seen_subcommand_from plugin' -a 'uninstall'  -d 'Remove an installed plugin'
complete -c creft -n '__fish_seen_subcommand_from plugin' -a 'activate'   -d 'Make plugin commands available in a scope'
complete -c creft -n '__fish_seen_subcommand_from plugin' -a 'deactivate' -d 'Remove plugin commands from a scope'
complete -c creft -n '__fish_seen_subcommand_from plugin' -a 'list'       -d 'List installed plugins'
complete -c creft -n '__fish_seen_subcommand_from plugin' -a 'search'     -d 'Search for commands across installed plugins'

# settings subcommands
complete -c creft -n '__fish_seen_subcommand_from settings' -a 'show' -d 'Show current settings'
complete -c creft -n '__fish_seen_subcommand_from settings' -a 'set'  -d 'Set a configuration value'

# completions subcommands (shell names)
complete -c creft -n '__fish_seen_subcommand_from completions' -a 'bash' -d 'Bash (4.0+)'
complete -c creft -n '__fish_seen_subcommand_from completions' -a 'zsh'  -d 'Zsh (5.0+)'
complete -c creft -n '__fish_seen_subcommand_from completions' -a 'fish' -d 'Fish (3.0+)'
"#
    .to_string()
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    /// Every generated script must call `creft list --names` so that completion picks
    /// up the current skill registry dynamically at tab-completion time.
    #[rstest]
    #[case::bash("bash", "creft list --names")]
    #[case::zsh("zsh", "creft list --names")]
    #[case::fish("fish", "creft list --names")]
    fn generated_script_calls_list_names(#[case] shell: &str, #[case] expected: &str) {
        let script = generate(shell).unwrap();
        assert!(
            script.contains(expected),
            "{shell} script must contain '{expected}'; got:\n{script}",
        );
    }

    /// Each shell's script must contain its shell-specific completion marker so that
    /// the shell's completion machinery picks up the function definition.
    #[rstest]
    #[case::bash("bash", "complete -F _creft creft")]
    #[case::zsh("zsh", "#compdef creft")]
    #[case::fish("fish", "complete -c creft")]
    fn generated_script_contains_shell_marker(#[case] shell: &str, #[case] marker: &str) {
        let script = generate(shell).unwrap();
        assert!(
            script.contains(marker),
            "{shell} script must contain '{marker}'; got:\n{script}",
        );
    }

    #[test]
    fn unsupported_shell_returns_cli_parse_error() {
        let err = generate("powershell").unwrap_err();
        assert!(
            matches!(err, CreftError::CliParse(_)),
            "unsupported shell must return CliParse error; got: {err:?}",
        );
    }

    #[test]
    fn unsupported_shell_error_names_the_shell() {
        let err = generate("powershell").unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("powershell"),
            "error message must name the unsupported shell; got: {msg:?}",
        );
    }

    #[test]
    fn bash_script_is_syntactically_valid() {
        let script = generate("bash").unwrap();
        let status = std::process::Command::new("bash")
            .args(["-n", "-c", &script])
            .status()
            .expect("bash must be available to validate bash syntax");
        assert!(
            status.success(),
            "bash completion script must pass 'bash -n' syntax check",
        );
    }

    #[test]
    fn zsh_script_is_syntactically_valid() {
        let Ok(zsh) = which_shell("zsh") else {
            return; // zsh not available in this environment; skip
        };
        let script = generate("zsh").unwrap();
        let status = std::process::Command::new(zsh)
            .args(["-n", "-c", &script])
            .status()
            .expect("failed to invoke zsh");
        assert!(
            status.success(),
            "zsh completion script must pass 'zsh -n' syntax check",
        );
    }

    #[test]
    fn fish_script_is_syntactically_valid() {
        let Ok(fish) = which_shell("fish") else {
            return; // fish not available in this environment; skip
        };
        let script = generate("fish").unwrap();
        let status = std::process::Command::new(fish)
            .args(["--no-execute", "-c", &script])
            .status()
            .expect("failed to invoke fish");
        assert!(
            status.success(),
            "fish completion script must pass 'fish --no-execute' syntax check",
        );
    }

    /// Returns the path to a shell binary if it exists on PATH.
    fn which_shell(name: &str) -> Result<String, ()> {
        let output = std::process::Command::new("which")
            .arg(name)
            .output()
            .map_err(|_| ())?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
        } else {
            Err(())
        }
    }
}
