use crate::error::CreftError;
use crate::model::AppContext;

/// Show current settings.
///
/// Stub for stage 5 — the settings subsystem is not yet implemented.
pub fn cmd_settings_show(_ctx: &AppContext) -> Result<(), CreftError> {
    eprintln!("no settings configured");
    Ok(())
}

/// Set a configuration value.
///
/// Stub for stage 5 — the settings subsystem is not yet implemented.
pub fn cmd_settings_set(_ctx: &AppContext, key: &str, _value: &str) -> Result<(), CreftError> {
    Err(CreftError::InvalidName(format!(
        "unknown setting '{key}'"
    )))
}
