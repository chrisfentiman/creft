use crate::error::CreftError;
use crate::model::AppContext;
use crate::setup;

pub fn cmd_up(ctx: &AppContext, system: Option<String>, local: bool) -> Result<(), CreftError> {
    let cwd = ctx.cwd.clone();

    if let Some(name) = system {
        let sys = setup::System::from_name(&name).ok_or_else(|| {
            CreftError::InvalidName(format!(
                "unknown system '{}'. supported: claude-code, cursor, windsurf, aider, copilot, codex, gemini",
                name
            ))
        })?;
        eprintln!(
            "installing creft instructions for {}...",
            sys.display_name()
        );
        let global = !local;
        setup::ensure_session_skill(ctx, &cwd, global)?;
        setup::install(ctx, sys, &cwd, global)?;
    } else if !local {
        // Default: install globally for all systems that support global install.
        // Aider global requires a manual config step, so it's excluded here.
        let global_systems = [
            setup::System::ClaudeCode,
            setup::System::Codex,
            setup::System::Gemini,
        ];
        eprintln!("installing creft instructions globally...");
        setup::ensure_session_skill(ctx, &cwd, true)?;
        for sys in &global_systems {
            eprintln!();
            eprintln!("{}:", sys.display_name());
            match setup::install(ctx, *sys, &cwd, true) {
                Ok(_) => {}
                Err(e) => eprintln!("  error: {}", e),
            }
        }
    } else {
        // --local: auto-detect systems in CWD and install per-project.
        let detected = setup::detect_systems(&cwd);
        if detected.is_empty() {
            eprintln!("no coding AI systems detected in current directory.");
            eprintln!("specify one explicitly: creft up <system>");
            eprintln!();
            eprintln!("supported systems:");
            for sys in setup::System::all() {
                eprintln!("  {:14} {}", sys.name(), sys.display_name());
            }
            return Ok(());
        }

        eprintln!(
            "detected {} system(s), installing creft instructions...",
            detected.len()
        );
        setup::ensure_session_skill(ctx, &cwd, false)?;
        for sys in &detected {
            eprintln!();
            eprintln!("{}:", sys.display_name());
            match setup::install(ctx, *sys, &cwd, false) {
                Ok(_) => {}
                Err(e) => eprintln!("  error: {}", e),
            }
        }
    }

    eprintln!();
    eprintln!("done. creft bootstraps itself at session start.");
    Ok(())
}
