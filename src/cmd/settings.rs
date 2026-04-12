use crate::error::CreftError;
use crate::model::AppContext;
use crate::settings::Settings;

/// Show all current settings.
///
/// Prints each setting as `key = value`. Prints "no settings configured"
/// when the settings file is absent or empty.
pub fn cmd_settings_show(ctx: &AppContext) -> Result<(), CreftError> {
    let path = ctx.settings_path()?;
    let settings = Settings::load(&path)?;

    let mut any = false;
    for (key, value) in settings.iter() {
        println!("{key} = {value}");
        any = true;
    }
    if !any {
        println!("no settings configured");
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
