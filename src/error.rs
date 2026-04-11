/// All error variants the creft CLI can produce.
#[derive(Debug, thiserror::Error)]
pub enum CreftError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("frontmatter parse error: {0}")]
    Frontmatter(String),

    #[error("missing frontmatter delimiter")]
    MissingFrontmatterDelimiter,

    #[error("command not found: {0}")]
    CommandNotFound(String),

    #[error("command already exists: {0}")]
    CommandAlreadyExists(String),

    #[error("reserved name: '{0}' is a built-in command and cannot be used")]
    ReservedName(String),

    #[error("missing required arg: {0}")]
    MissingArg(String),

    #[error("validation failed for '{name}': value '{value}' does not match pattern '{pattern}'")]
    ValidationFailed {
        name: String,
        value: String,
        pattern: String,
    },

    #[error("missing required env var: {0}")]
    MissingEnvVar(String),

    #[error("no code blocks found in command definition")]
    NoCodeBlocks,

    #[error("command failed: block {block} ({lang}) exited with code {code}")]
    ExecutionFailed {
        block: usize,
        lang: String,
        code: i32,
    },

    #[error("block {block} ({lang}) was killed by signal {signal}")]
    ExecutionSignaled {
        block: usize,
        lang: String,
        signal: i32,
    },

    #[error("interpreter not found: {0}")]
    InterpreterNotFound(String),

    #[error("invalid command name: {0}")]
    InvalidName(String),

    #[error("setup error: {0}")]
    Setup(String),

    #[error("serialization error: {0}")]
    Serialization(String),

    #[error("git command failed: {0}")]
    Git(String),

    #[error("package not found: {0}")]
    PackageNotFound(String),

    #[error("package already installed: {0}")]
    PackageAlreadyInstalled(String),

    #[error("invalid manifest: {0}")]
    InvalidManifest(String),

    #[error("manifest not found in repository (expected .creft/catalog.json at repo root)")]
    ManifestNotFound,

    #[error("activation not found: '{cmd}' is not activated in plugin '{plugin}'")]
    ActivationNotFound { plugin: String, cmd: String },

    #[error("catalog parse error in '{catalog_source}': {detail}")]
    CatalogParse {
        catalog_source: String,
        detail: String,
    },

    #[error("plugin '{plugin}' not found in catalog '{catalog}' (available: {available})")]
    PluginNotInCatalog {
        catalog: String,
        plugin: String,
        available: String,
    },

    #[error("validation failed")]
    ValidationErrors(Vec<crate::validate::ValidationDiagnostic>),

    /// A block exited with code 99: stop the pipeline and return success.
    ///
    /// This variant is an internal signal used by the runner. It is never
    /// surfaced to the user — callers translate it to `Ok(())`.
    #[error("early exit (exit 99)")]
    EarlyExit,
}

impl CreftError {
    /// Map this error to a process exit code.
    ///
    /// Follows Unix conventions: 2 for "not found", 3 for bad input, the
    /// child's own exit code for `ExecutionFailed`, 128 + signal for
    /// `ExecutionSignaled`, and 1 for everything else.
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::CommandNotFound(_)
            | Self::PackageNotFound(_)
            | Self::ActivationNotFound { .. }
            | Self::PluginNotInCatalog { .. } => 2,
            Self::MissingArg(_) | Self::MissingEnvVar(_) | Self::ValidationFailed { .. } => 3,
            Self::ExecutionFailed { code, .. } => *code,
            Self::ExecutionSignaled { signal, .. } => 128 + signal,
            _ => 1,
        }
    }

    /// Returns `true` if this error should not be printed to stderr.
    ///
    /// Quiet errors are those where the child process already communicated
    /// the failure via its own stderr (which goes directly to the terminal).
    /// Printing creft's wrapper message would be redundant.
    ///
    /// The exit code is still set correctly -- only the message is suppressed.
    pub fn is_quiet(&self) -> bool {
        match self {
            Self::ExecutionFailed { .. } => true,
            // SIGINT (signal 2): user pressed Ctrl+C. They know what happened.
            // Don't print "block N (bash) was killed by signal 2" -- it's noise.
            // The exit code (130) still signals the death to the parent shell.
            Self::ExecutionSignaled { signal, .. } if *signal == 2 => true,
            _ => false,
        }
    }
}

/// Wrap an `io::Error` with context-specific actionable guidance.
///
/// Pattern-matches on the raw OS error code and `ErrorKind` to return a
/// `CreftError` variant with a more helpful message than the raw OS error.
///
/// - E2BIG (os error 7): argument list too large — output exceeds OS env limit.
/// - `NotFound` during spawn: interpreter missing — suggest `creft doctor`.
/// - All other cases: `CreftError::Io(e)` unchanged.
pub fn enrich_io_error(e: std::io::Error, context: &str) -> CreftError {
    if e.raw_os_error() == Some(7) {
        return CreftError::Setup(format!(
            "Output too large for environment variable ({context}). \
            The output exceeded the OS argument size limit."
        ));
    }
    if e.kind() == std::io::ErrorKind::NotFound {
        return CreftError::InterpreterNotFound(format!("{context}. Run 'creft doctor' to check."));
    }
    CreftError::Io(e)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[allow(unused_imports)]
    use pretty_assertions::{assert_eq, assert_ne};

    // --- exit_code() tests ---

    #[test]
    fn test_exit_code_command_not_found() {
        let err = CreftError::CommandNotFound("foo".to_string());
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn test_exit_code_package_not_found() {
        let err = CreftError::PackageNotFound("some-pkg".to_string());
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn test_exit_code_missing_arg() {
        let err = CreftError::MissingArg("name".to_string());
        assert_eq!(err.exit_code(), 3);
    }

    #[test]
    fn test_exit_code_missing_env_var() {
        let err = CreftError::MissingEnvVar("API_KEY".to_string());
        assert_eq!(err.exit_code(), 3);
    }

    #[test]
    fn test_exit_code_validation_failed() {
        let err = CreftError::ValidationFailed {
            name: "port".to_string(),
            value: "abc".to_string(),
            pattern: r"\d+".to_string(),
        };
        assert_eq!(err.exit_code(), 3);
    }

    #[test]
    fn test_exit_code_execution_failed_returns_code_field() {
        let err = CreftError::ExecutionFailed {
            block: 1,
            lang: "bash".to_string(),
            code: 42,
        };
        assert_eq!(err.exit_code(), 42);
    }

    #[test]
    fn test_exit_code_execution_failed_zero_code() {
        // A caller can construct ExecutionFailed with code 0; exit_code reflects it.
        let err = CreftError::ExecutionFailed {
            block: 0,
            lang: "python".to_string(),
            code: 0,
        };
        assert_eq!(err.exit_code(), 0);
    }

    #[test]
    fn test_exit_code_execution_signaled() {
        // 128 + 15 (SIGTERM) = 143
        let err = CreftError::ExecutionSignaled {
            block: 0,
            lang: "bash".to_string(),
            signal: 15,
        };
        assert_eq!(err.exit_code(), 143);
    }

    #[test]
    fn test_exit_code_wildcard_arm_returns_1_for_io() {
        let io_err = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied");
        let err = CreftError::Io(io_err);
        assert_eq!(err.exit_code(), 1);
    }

    #[test]
    fn test_exit_code_wildcard_arm_returns_1_for_setup() {
        let err = CreftError::Setup("some setup failure".to_string());
        assert_eq!(err.exit_code(), 1);
    }

    #[test]
    fn test_exit_code_wildcard_arm_returns_1_for_frontmatter() {
        let err = CreftError::Frontmatter("bad yaml".to_string());
        assert_eq!(err.exit_code(), 1);
    }

    // --- is_quiet() tests ---

    #[test]
    fn test_is_quiet_execution_failed() {
        let err = CreftError::ExecutionFailed {
            block: 1,
            lang: "bash".to_string(),
            code: 1,
        };
        assert!(err.is_quiet());
    }

    #[test]
    fn test_is_quiet_execution_signaled_sigterm() {
        // SIGTERM (15) is unexpected — creft should report it.
        let err = CreftError::ExecutionSignaled {
            block: 0,
            lang: "bash".to_string(),
            signal: 15,
        };
        assert!(!err.is_quiet());
    }

    #[test]
    fn test_is_quiet_execution_signaled_sigint() {
        // SIGINT (2): user pressed Ctrl+C — suppress creft's own error message.
        let err = CreftError::ExecutionSignaled {
            block: 0,
            lang: "bash".to_string(),
            signal: 2,
        };
        assert!(err.is_quiet());
    }

    #[test]
    fn test_is_quiet_execution_signaled_sigkill() {
        // SIGKILL (9) is unexpected — creft should report it.
        let err = CreftError::ExecutionSignaled {
            block: 0,
            lang: "bash".to_string(),
            signal: 9,
        };
        assert!(!err.is_quiet());
    }

    #[test]
    fn test_is_quiet_command_not_found() {
        let err = CreftError::CommandNotFound("foo".to_string());
        assert!(!err.is_quiet());
    }

    #[test]
    fn test_is_quiet_io() {
        let io_err = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied");
        let err = CreftError::Io(io_err);
        assert!(!err.is_quiet());
    }

    // --- enrich_io_error() tests ---

    #[test]
    fn test_enrich_io_error_e2big_returns_setup() {
        // Simulate E2BIG (raw OS error 7).
        let e2big = std::io::Error::from_raw_os_error(7);
        let result = enrich_io_error(e2big, "environment");
        assert!(
            matches!(result, CreftError::Setup(ref msg) if msg.contains("OS argument size limit")),
            "Expected Setup variant with OS argument size limit message"
        );
    }

    #[test]
    fn test_enrich_io_error_not_found_returns_interpreter_not_found() {
        let not_found = std::io::Error::new(std::io::ErrorKind::NotFound, "no such file");
        let result = enrich_io_error(not_found, "bash");
        assert!(
            matches!(result, CreftError::InterpreterNotFound(ref msg) if msg.contains("creft doctor")),
            "Expected InterpreterNotFound variant with creft doctor hint"
        );
    }

    #[test]
    fn test_enrich_io_error_not_found_includes_context() {
        let not_found = std::io::Error::new(std::io::ErrorKind::NotFound, "no such file");
        let result = enrich_io_error(not_found, "python3");
        match result {
            CreftError::InterpreterNotFound(msg) => {
                assert!(
                    msg.contains("python3"),
                    "Context should appear in the message"
                );
            }
            other => panic!("Expected InterpreterNotFound, got {other:?}"),
        }
    }

    #[test]
    fn test_enrich_io_error_other_passthrough_as_io() {
        let permission_denied = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied");
        let result = enrich_io_error(permission_denied, "ctx");
        assert!(
            matches!(result, CreftError::Io(_)),
            "Expected Io passthrough for non-E2BIG, non-NotFound errors"
        );
    }

    #[test]
    fn test_enrich_io_error_e2big_context_appears_in_message() {
        let e2big = std::io::Error::from_raw_os_error(7);
        let result = enrich_io_error(e2big, "MY_VAR");
        match result {
            CreftError::Setup(msg) => {
                assert!(
                    msg.contains("MY_VAR"),
                    "Context should appear in E2BIG message"
                );
            }
            other => panic!("Expected Setup, got {other:?}"),
        }
    }
}
