use crate::doctor;
use crate::error::CreftError;
use crate::model::AppContext;
use crate::settings::Settings;
use crate::shell;
use crate::store;

pub fn cmd_doctor(ctx: &AppContext, name: Vec<String>) -> Result<(), CreftError> {
    if name.is_empty() {
        let results = doctor::run_global_check(ctx);
        doctor::render_global(&results);
        if doctor::has_failures(&results) {
            std::process::exit(1);
        }
        Ok(())
    } else {
        // Load the shell preference so the skill check can report the resolved
        // interpreter for shell-family blocks. A missing or corrupt settings
        // file is treated as no preference — the check itself must not fail.
        let settings_shell_pref = ctx
            .settings_path()
            .ok()
            .and_then(|p| Settings::load(&p).ok())
            .and_then(|s| s.get("shell").map(str::to_string));
        let shell_pref = shell::detect(settings_shell_pref.as_deref());

        let (resolved_name, _, source) = store::resolve_command(ctx, &name)?;
        let report = doctor::run_skill_check(ctx, &resolved_name, &source, shell_pref.as_deref())?;
        doctor::render_skill(&report);
        if doctor::report_has_failures(&report) {
            std::process::exit(1);
        }
        Ok(())
    }
}
