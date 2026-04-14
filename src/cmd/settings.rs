use crate::error::CreftError;
use crate::model::AppContext;
use crate::settings::{SettingValue, Settings};

/// Show all known settings with their current or default values.
///
/// Prints one line per known key in the form `key = value` when configured,
/// or `key = (default: <description>)` when using the runtime default.
pub fn cmd_settings_show(ctx: &AppContext) -> Result<(), CreftError> {
    let path = ctx.settings_path()?;
    let settings = Settings::load(&path)?;

    for (key, value) in settings.known_entries() {
        match value {
            SettingValue::Set(v) => println!("{key} = {v}"),
            SettingValue::Default(desc) => println!("{key} = (default: {desc})"),
        }
    }
    Ok(())
}

/// Set a configuration value.
///
/// Persists the key/value pair to the settings file. Returns an error for
/// unknown keys.
pub fn cmd_settings_set(ctx: &AppContext, key: &str, value: &str) -> Result<(), CreftError> {
    let path = ctx.settings_path()?;
    let mut settings = Settings::load(&path)?;
    settings.set(key, value)?;
    settings.save(&path)?;
    println!("set {key} = {value}");
    Ok(())
}
