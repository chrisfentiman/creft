use crate::doctor;
use crate::error::CreftError;
use crate::model::AppContext;
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
        let (resolved_name, _, source) = store::resolve_command(ctx, &name)?;
        let report = doctor::run_skill_check(ctx, &resolved_name, &source)?;
        doctor::render_skill(&report);
        if doctor::report_has_failures(&report) {
            std::process::exit(1);
        }
        Ok(())
    }
}
