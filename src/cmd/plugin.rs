use yansi::Paint;

use crate::cmd::skill::{LIST_DESC_MAX, truncate_desc};
use crate::error::CreftError;
use crate::model::AppContext;
use crate::{model, registry};

pub fn cmd_plugin_install(
    ctx: &AppContext,
    source: &str,
    plugin: Option<&str>,
) -> Result<(), CreftError> {
    let pkg = if source.contains("://") || source.starts_with("git@") || source.starts_with('/') {
        // Full URL or absolute path: clone directly.
        registry::plugin_install(ctx, source, plugin)?
    } else if source.contains('/') {
        // Shorthand name (owner/repo or creft/plugin): resolve via install_by_name.
        registry::install_by_name(ctx, source, plugin)?
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
    Ok(())
}

pub fn cmd_plugin_deactivate(
    ctx: &AppContext,
    target: &str,
    global: bool,
) -> Result<(), CreftError> {
    registry::deactivate(ctx, target, global)?;
    eprintln!("deactivated: {target}");
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

    for m in &matches {
        let desc = truncate_desc(&m.def.description, LIST_DESC_MAX);
        let pad = " ".repeat(max_name - m.def.name.len());
        println!(
            "  {}{}  {}  (plugin: {})",
            m.def.name.as_str().bold(),
            pad,
            desc,
            m.plugin_name
        );
    }

    Ok(())
}
