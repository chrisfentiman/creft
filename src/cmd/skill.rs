use std::borrow::Cow;
use std::io::{IsTerminal, Read};

use crate::error::CreftError;
use crate::model::AppContext;
use crate::{frontmatter, markdown, model, store, style, validate};

/// Maximum description characters shown in list output.
/// Matches the visual width used by cargo's command listing.
pub const LIST_DESC_MAX: usize = 60;

#[allow(clippy::too_many_arguments)]
pub fn cmd_add(
    ctx: &AppContext,
    name_override: Option<String>,
    desc_override: Option<String>,
    arg_defs: Vec<String>,
    tags: Vec<String>,
    force: bool,
    no_validate: bool,
    global: bool,
) -> Result<(), CreftError> {
    let mut input = String::new();

    if std::io::stdin().is_terminal() {
        let name = name_override.ok_or_else(|| {
            CreftError::InvalidName("--name is required when not piping from stdin".into())
        })?;
        let desc = desc_override.unwrap_or_default();

        let mut args_yaml = String::new();
        for arg_str in &arg_defs {
            let parts: Vec<&str> = arg_str.splitn(2, ':').collect();
            let arg_name = parts[0].trim();
            let arg_desc = parts.get(1).map(|s| s.trim()).unwrap_or("");
            args_yaml.push_str(&format!(
                "  - name: {}\n    description: {}\n",
                arg_name, arg_desc
            ));
        }

        let mut tags_yaml = String::new();
        if !tags.is_empty() {
            tags_yaml = format!("tags: [{}]\n", tags.join(", "));
        }

        input = format!(
            "---\nname: {}\ndescription: {}\n{}{}---\n",
            name,
            desc,
            if args_yaml.is_empty() {
                String::new()
            } else {
                format!("args:\n{}", args_yaml)
            },
            tags_yaml,
        );
    } else {
        std::io::stdin()
            .read_to_string(&mut input)
            .map_err(CreftError::Io)?;

        if name_override.is_some()
            || desc_override.is_some()
            || !arg_defs.is_empty()
            || !tags.is_empty()
        {
            let (mut def, body) = frontmatter::parse(&input)?;
            if let Some(n) = name_override {
                def.name = n;
            }
            if let Some(d) = desc_override {
                def.description = d;
            }
            input = frontmatter::serialize(&def, &body)?;
        }
    }

    if !force && !no_validate {
        let (def, body) = frontmatter::parse(&input)?;
        let (_, blocks) = markdown::extract_blocks(&body);
        let result = validate::validate_skill(&def, &blocks, Some(ctx));

        for w in &result.warnings {
            eprintln!("warning: {}", w);
        }

        if result.has_errors() {
            for e in &result.errors {
                eprintln!("error: {}", e);
            }
            return Err(CreftError::ValidationErrors(result.errors));
        }
    }

    let scope = if global {
        model::Scope::Global
    } else {
        ctx.default_write_scope()
    };
    let name = store::save(ctx, &input, force, scope)?;
    eprintln!("added: {}", name);
    Ok(())
}

pub fn cmd_list(
    ctx: &AppContext,
    tag: Option<String>,
    show_all: bool,
    namespace: Vec<String>,
) -> Result<(), CreftError> {
    use std::collections::{HashMap, HashSet};

    let all = store::list_all_with_source(ctx)?;

    if all.is_empty() {
        eprintln!("no commands found. use 'creft cmd add' to create one.");
        return Ok(());
    }

    let prefix: Vec<&str> = namespace.iter().map(|s| s.as_str()).collect();

    // Check the unfiltered list so a tag filter that empties a valid namespace doesn't
    // produce a misleading "no skills found under '...'" message.
    if !prefix.is_empty() {
        let namespace_exists = all.iter().any(|(def, _)| {
            let parts = def.name_parts();
            parts.len() > prefix.len()
                && parts[..prefix.len()]
                    .iter()
                    .zip(prefix.iter())
                    .all(|(a, b)| a == b)
        });
        if !namespace_exists {
            eprintln!("no skills found under '{}'", prefix.join(" "));
            return Ok(());
        }
    }

    let tag_filtered: Vec<_> = if let Some(ref t) = tag {
        all.into_iter()
            .filter(|(d, _)| d.tags.iter().any(|dt| dt == t))
            .collect()
    } else {
        all
    };

    if tag_filtered.is_empty() {
        eprintln!("no commands found. use 'creft cmd add' to create one.");
        return Ok(());
    }

    // Suppress hidden commands unless the user explicitly named a hidden prefix or
    // passed --all (which opts into seeing everything).
    let explicit_hidden = prefix.iter().any(|p| p.starts_with('_'));
    let visible: Vec<_> = if explicit_hidden || show_all {
        tag_filtered
    } else {
        tag_filtered
            .into_iter()
            .filter(|(def, _)| !def.is_hidden())
            .collect()
    };

    if show_all {
        let flat: Vec<_> = if prefix.is_empty() {
            visible
        } else {
            visible
                .into_iter()
                .filter(|(def, _)| {
                    let parts = def.name_parts();
                    parts.len() > prefix.len()
                        && parts[..prefix.len()]
                            .iter()
                            .zip(prefix.iter())
                            .all(|(a, b)| a == b)
                })
                .collect()
        };

        if flat.is_empty() {
            eprintln!("no commands found. use 'creft cmd add' to create one.");
            return Ok(());
        }

        let ansi = style::use_ansi();
        println!("{}", style::bold("Skills:", ansi));
        println!();

        let max_name = flat.iter().map(|(d, _)| d.name.len()).max().unwrap_or(0);

        for (def, source) in &flat {
            let desc = format_skill_desc(def, source, LIST_DESC_MAX);
            let pad = " ".repeat(max_name - def.name.len());
            println!("  {}{}  {}", style::bold(&def.name, ansi), pad, desc);
        }
        return Ok(());
    }

    let entries = store::group_by_namespace(visible, &prefix);

    if entries.is_empty() {
        eprintln!("no commands found. use 'creft cmd add' to create one.");
        return Ok(());
    }

    let ansi = style::use_ansi();

    if prefix.is_empty() {
        println!("{}", style::bold(crate::help::ROOT_ABOUT, ansi));
        println!();
        println!(
            "{}creft <skill> [ARGS] [OPTIONS]",
            style::bold("Usage: ", ansi)
        );
        println!();
        println!("{}", style::bold("Skills:", ansi));
    } else {
        let header = format!("Skills in '{}':", prefix.join(" "));
        println!("{}", style::bold(&header, ansi));
    }
    println!();

    // When a leaf skill and a namespace share the same relative name, suppress
    // the namespace entry and annotate the leaf with "[N subskills]" instead.
    let mut namespace_map: HashMap<String, (usize, Option<String>)> = HashMap::new();
    let mut leaf_names: HashSet<String> = HashSet::new();

    for entry in &entries {
        match entry {
            model::NamespaceEntry::Namespace {
                name,
                skill_count,
                package,
            } => {
                let relative = name
                    .split_whitespace()
                    .next_back()
                    .unwrap_or(name.as_str())
                    .to_string();
                namespace_map.insert(relative, (*skill_count, package.clone()));
            }
            model::NamespaceEntry::Skill(def, _) => {
                let parts = def.name_parts();
                if let Some(relative) = parts.get(prefix.len()) {
                    leaf_names.insert((*relative).to_string());
                }
            }
        }
    }

    // Exclude suppressed namespace entries from column-width computation.
    let max_name = entries
        .iter()
        .filter_map(|e| match e {
            model::NamespaceEntry::Skill(def, _) => Some(def.name.len()),
            model::NamespaceEntry::Namespace { name, .. } => {
                let relative = name.split_whitespace().next_back().unwrap_or(name.as_str());
                if leaf_names.contains(relative) {
                    None
                } else {
                    Some(name.len())
                }
            }
        })
        .max()
        .unwrap_or(0);

    for entry in &entries {
        match entry {
            model::NamespaceEntry::Skill(def, source) => {
                let desc = format_skill_desc(def, source, LIST_DESC_MAX);
                let parts = def.name_parts();
                let relative = parts
                    .get(prefix.len())
                    .map(|s| s.to_string())
                    .unwrap_or_default();
                let suffix = if let Some((count, _)) = namespace_map.get(&relative) {
                    let label = if *count == 1 { "subskill" } else { "subskills" };
                    format!("  [{count} {label}]")
                } else {
                    String::new()
                };
                let pad = " ".repeat(max_name - def.name.len());
                println!("  {}{}  {desc}{suffix}", style::bold(&def.name, ansi), pad);
            }
            model::NamespaceEntry::Namespace {
                name,
                skill_count,
                package,
            } => {
                let relative = name.split_whitespace().next_back().unwrap_or(name.as_str());
                if leaf_names.contains(relative) {
                    continue;
                }
                let plural = if *skill_count == 1 { "skill" } else { "skills" };
                let pkg_suffix = if package.is_some() { "  [package]" } else { "" };
                let pad = " ".repeat(max_name - name.len());
                println!(
                    "  {}{}  {skill_count} {plural}{pkg_suffix}",
                    style::bold(name, ansi),
                    pad,
                );
            }
        }
    }

    if prefix.is_empty() {
        println!();
        println!("See 'creft <skill> --help' for details.");
    }

    Ok(())
}

pub fn cmd_show(ctx: &AppContext, name: &str) -> Result<(), CreftError> {
    let args: Vec<String> = name.split_whitespace().map(String::from).collect();
    let (resolved_name, _, source) = store::resolve_command(ctx, &args)?;
    let content = store::read_raw_from(ctx, &resolved_name, &source)?;
    println!("{}", content);
    Ok(())
}

pub fn cmd_cat(ctx: &AppContext, name: &str) -> Result<(), CreftError> {
    let args: Vec<String> = name.split_whitespace().map(String::from).collect();
    let (resolved_name, _, source) = store::resolve_command(ctx, &args)?;
    let cmd = store::load_from(ctx, &resolved_name, &source)?;
    for block in &cmd.blocks {
        println!("{}", block.code);
    }
    Ok(())
}

pub fn cmd_rm(ctx: &AppContext, name: &str, global: bool) -> Result<(), CreftError> {
    let args: Vec<String> = name.split_whitespace().map(String::from).collect();
    let (_, _, source) = store::resolve_command(ctx, &args)?;

    match &source {
        model::SkillSource::Package(_, _) => {
            return Err(CreftError::Setup(
                "cannot remove individual skills from an installed package -- use 'creft plugins uninstall <package>' instead".into(),
            ));
        }
        model::SkillSource::Plugin(_) => {
            return Err(CreftError::Setup(
                "cannot remove individual skills from a plugin -- use 'creft plugins deactivate <plugin>' or 'creft plugins uninstall <plugin>' instead".into(),
            ));
        }
        model::SkillSource::Owned(_) => {}
    }

    let scope = if global {
        model::Scope::Global
    } else {
        match &source {
            model::SkillSource::Owned(s) => *s,
            model::SkillSource::Package(_, s) => *s,
            // Unreachable: Plugin is rejected above, but required for exhaustiveness.
            model::SkillSource::Plugin(_) => model::Scope::Global,
        }
    };
    store::remove_in(ctx, name, scope)?;
    eprintln!("removed: {}", name);
    Ok(())
}

/// Truncate a string to `max_len` characters, appending "..." if truncated.
/// Operates on character count, not byte count (handles Unicode).
pub fn truncate_desc(s: &str, max_len: usize) -> Cow<'_, str> {
    if s.chars().count() <= max_len {
        Cow::Borrowed(s)
    } else {
        let truncated: String = s.chars().take(max_len.saturating_sub(3)).collect();
        Cow::Owned(format!("{}...", truncated.trim_end()))
    }
}

/// Format the description column for a flat skill entry.
///
/// Scope annotations (`(local)` / `(global)`) are omitted — they are noise
/// for discovery. Package provenance is preserved as `(pkg: <name>)` because
/// it is actionable (users can uninstall or inspect package skills separately).
/// The description is truncated to `max_desc_len` characters before appending
/// any package annotation so the annotation is never truncated.
pub fn format_skill_desc(
    def: &model::CommandDef,
    source: &model::SkillSource,
    max_desc_len: usize,
) -> String {
    let desc = truncate_desc(&def.description, max_desc_len);
    match source {
        model::SkillSource::Owned(_) => desc.into_owned(),
        model::SkillSource::Package(pkg, _) => format!("{desc}  (pkg: {pkg})"),
        model::SkillSource::Plugin(name) => format!("{desc}  (plugin: {name})"),
    }
}
