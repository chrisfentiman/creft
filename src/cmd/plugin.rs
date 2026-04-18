use yansi::Paint;

use crate::error::CreftError;
use crate::model::AppContext;
use crate::search;
use crate::wrap::{MAX_WIDTH, wrap_description};
use crate::{model, registry};

pub fn cmd_plugin_install(ctx: &AppContext, source: &str) -> Result<(), CreftError> {
    let pkg = if source.contains("://") || source.starts_with("git@") || source.starts_with('/') {
        // Full URL or absolute path: clone directly. The repo must contain
        // exactly one plugin; multi-plugin repos require the owner/plugin
        // shorthand format (e.g. `creft plugin install creft/ask`).
        registry::plugin_install(ctx, source, None)?
    } else if source.contains('/') {
        // Shorthand name (owner/repo or creft/plugin): derive plugin name from path.
        registry::install_by_name(ctx, source)?
    } else {
        return Err(CreftError::InvalidManifest(format!(
            "'{source}' is not a valid plugin source — use owner/repo or a full git URL"
        )));
    };
    eprintln!(
        "installed: {} ({})",
        pkg.manifest.name, pkg.manifest.version
    );
    Ok(())
}

pub fn cmd_plugin_update(ctx: &AppContext, name: Option<String>) -> Result<(), CreftError> {
    match name {
        Some(n) => {
            let pkg = registry::plugin_update(ctx, &n)?;
            eprintln!("updated: {} ({})", pkg.manifest.name, pkg.manifest.version);
        }
        None => {
            let results = registry::plugin_update_all(ctx)?;
            if results.is_empty() {
                eprintln!("no plugins installed");
                return Ok(());
            }
            for result in results {
                match result {
                    Ok(pkg) => {
                        eprintln!("updated: {} ({})", pkg.manifest.name, pkg.manifest.version)
                    }
                    Err(e) => eprintln!("error: {}", e),
                }
            }
        }
    }
    Ok(())
}

pub fn cmd_plugin_uninstall(ctx: &AppContext, name: &str) -> Result<(), CreftError> {
    registry::plugin_uninstall(ctx, name)?;
    eprintln!("uninstalled: {}", name);
    Ok(())
}

pub fn cmd_plugin_activate(ctx: &AppContext, target: &str, global: bool) -> Result<(), CreftError> {
    let scope = if global {
        model::Scope::Global
    } else {
        ctx.default_write_scope()
    };
    registry::activate(ctx, target, scope)?;
    eprintln!("activated: {target}");

    // Rebuild indexes so activated plugin skills are immediately searchable.
    // A failed rebuild does not prevent the activation from succeeding.
    if let Err(e) = search::store::rebuild_all_indexes(ctx) {
        eprintln!("warning: could not rebuild search indexes after activation: {e}");
    }

    Ok(())
}

pub fn cmd_plugin_deactivate(
    ctx: &AppContext,
    target: &str,
    global: bool,
) -> Result<(), CreftError> {
    registry::deactivate(ctx, target, global)?;
    eprintln!("deactivated: {target}");

    // Rebuild indexes so deactivated plugin skills no longer appear in search.
    // A failed rebuild does not prevent the deactivation from succeeding.
    if let Err(e) = search::store::rebuild_all_indexes(ctx) {
        eprintln!("warning: could not rebuild search indexes after deactivation: {e}");
    }

    Ok(())
}

pub fn cmd_plugin_list(ctx: &AppContext, name: Option<&str>) -> Result<(), CreftError> {
    let plugins_dir = ctx.plugins_dir()?;
    if !plugins_dir.exists() {
        eprintln!("no plugins installed");
        return Ok(());
    }

    match name {
        Some(plugin_name) => {
            let plugin_dir = plugins_dir.join(plugin_name);
            if !plugin_dir.exists() {
                return Err(CreftError::PackageNotFound(plugin_name.to_string()));
            }
            let skills = registry::list_plugin_skills_in(ctx, plugin_name)?;
            if skills.is_empty() {
                eprintln!("no commands found in plugin '{}'", plugin_name);
            } else {
                for skill in &skills {
                    println!("{}", skill.name);
                }
            }
        }
        None => {
            let mut names: Vec<String> = std::fs::read_dir(&plugins_dir)?
                .filter_map(|entry| {
                    let entry = entry.ok()?;
                    if entry.file_type().ok()?.is_dir() {
                        entry.file_name().into_string().ok()
                    } else {
                        None
                    }
                })
                .collect();
            names.sort();
            if names.is_empty() {
                eprintln!("no plugins installed");
            } else {
                for name in &names {
                    println!("{}", name);
                }
            }
        }
    }
    Ok(())
}

pub fn cmd_plugin_search(ctx: &AppContext, query: &[String]) -> Result<(), CreftError> {
    let matches = registry::plugin_search(ctx, query)?;

    if matches.is_empty() {
        if query.is_empty() {
            eprintln!("no plugins installed");
        } else {
            eprintln!("no matching skills found");
        }
        return Ok(());
    }

    let max_name = matches.iter().map(|m| m.def.name.len()).max().unwrap_or(0);
    let desc_col = 2 + max_name + 2;
    let desc_budget = MAX_WIDTH.saturating_sub(desc_col);

    for m in &matches {
        let raw = format!("{}  (plugin: {})", m.def.description, m.plugin_name);
        let desc = wrap_description(&raw, desc_budget, desc_col);
        let pad = " ".repeat(max_name - m.def.name.len());
        println!("  {}{}  {}", m.def.name.as_str().bold(), pad, desc);
    }

    Ok(())
}
