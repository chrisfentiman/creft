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

        match store::resolve_command(ctx, &name) {
            Ok((resolved_name, _, source)) => {
                let report =
                    doctor::run_skill_check(ctx, &resolved_name, &source, shell_pref.as_deref())?;
                doctor::render_skill(&report);
                if doctor::report_has_failures(&report) {
                    std::process::exit(1);
                }
                Ok(())
            }
            Err(e) => {
                // Namespace mode: `creft doctor <ns>` checks every skill in the
                // namespace and renders a report per skill.
                let prefix: Vec<&str> = name.iter().map(|s| s.as_str()).collect();
                if !store::namespace_exists(ctx, &prefix)? {
                    return Err(e);
                }
                let skills = store::list_namespace_skills(ctx, &prefix)?;
                if skills.is_empty() {
                    return Err(e);
                }
                let mut any_failure = false;
                for (def, source) in &skills {
                    match doctor::run_skill_check(ctx, &def.name, source, shell_pref.as_deref()) {
                        Ok(report) => {
                            doctor::render_skill(&report);
                            if doctor::report_has_failures(&report) {
                                any_failure = true;
                            }
                        }
                        Err(check_err) => {
                            eprintln!("error checking {}: {check_err}", def.name);
                            any_failure = true;
                        }
                    }
                }
                if any_failure {
                    std::process::exit(1);
                }
                Ok(())
            }
        }
    }
}
