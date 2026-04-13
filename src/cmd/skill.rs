use std::borrow::Cow;
use std::io::{IsTerminal, Read};

use yansi::Paint;

use crate::error::CreftError;
use crate::model::AppContext;
use crate::{frontmatter, help, markdown, model, store, style, validate};

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
    let all = store::list_all_with_source(ctx)?;
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
        // Flat listing: render all skills without grouping.
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
            eprintln!("no commands found. use 'creft add' to create one.");
            return Ok(());
        }

        println!("{}", "Skills:".bold());
        println!();

        let max_name = flat.iter().map(|(d, _)| d.name.len()).max().unwrap_or(0);

        for (def, source) in &flat {
            let desc = format_skill_desc(def, source, LIST_DESC_MAX);
            let pad = " ".repeat(max_name - def.name.len());
            println!("  {}{}  {}", def.name.as_str().bold(), pad, desc);
        }
        return Ok(());
    }

    let entries = store::group_by_namespace(visible, &prefix);

    if prefix.is_empty() {
        // Root listing: Commands: section + Skills: section with truncation.
        print_root_listing(&entries, /* display_limit */ compute_display_limit());
    } else {
        // Namespace drill-in: skills in the requested namespace only, no builtins.
        if entries.is_empty() {
            eprintln!("no commands found. use 'creft add' to create one.");
            return Ok(());
        }
        let header = format!("Skills in '{}':", prefix.join(" "));
        println!("{}", header.as_str().bold());
        println!();
        print_skill_entries(&entries, &prefix, None);
    }

    Ok(())
}

/// Compute the display limit for the Skills section based on terminal height.
///
/// Returns `None` when stdout is not a TTY (no truncation for piped output).
fn compute_display_limit() -> Option<usize> {
    let rows = style::terminal_rows()?;
    // ~20 lines consumed by tagline, usage, Commands: header, 10 builtins, blank
    // lines, Skills: header, flags, footer.
    let limit = (rows as usize).saturating_sub(20).max(20);
    Some(limit)
}

/// Print the root listing: tagline, usage, Commands: section, then Skills: section.
fn print_root_listing(entries: &[model::NamespaceEntry], display_limit: Option<usize>) {
    println!("{}", help::ROOT_ABOUT.bold());
    println!();
    println!("{}creft <command> [ARGS] [OPTIONS]", "Usage: ".bold());
    println!();

    // Commands: section — fixed list of builtins, always shown in full.
    println!("{}", "Commands:".bold());
    let builtins = help::builtins();
    let max_builtin_name = builtins.iter().map(|e| e.name.len()).max().unwrap_or(0);
    for entry in builtins {
        let pad = " ".repeat(max_builtin_name - entry.name.len());
        println!("  {}{}  {}", entry.name.bold(), pad, entry.description);
    }

    if entries.is_empty() {
        // No skills installed — omit the Skills: section entirely.
        println!();
        print_global_flags(max_builtin_name);
        println!();
        println!("See 'creft <command> --help' for details.");
        return;
    }

    // Skills: section.
    println!();
    println!("{}", "Skills:".bold());
    println!();

    let empty_prefix: &[&str] = &[];
    let total = count_visible_entries(entries);
    let limit = display_limit.unwrap_or(usize::MAX);

    print_skill_entries_limited(entries, empty_prefix, limit);

    if total > limit {
        println!("  (showing {limit} of {total} — use 'creft list --all' to see all)");
    }

    println!();
    print_global_flags(max_builtin_name);
    println!();
    println!("See 'creft <command> --help' for details.");
}

/// Count the number of visible (non-suppressed) entries in a namespace group.
fn count_visible_entries(entries: &[model::NamespaceEntry]) -> usize {
    use std::collections::HashSet;

    let mut leaf_names: HashSet<&str> = HashSet::new();
    for entry in entries {
        if let model::NamespaceEntry::Skill(def, _) = entry {
            let parts = def.name_parts();
            if let Some(relative) = parts.first() {
                leaf_names.insert(relative);
            }
        }
    }

    entries
        .iter()
        .filter(|e| match e {
            model::NamespaceEntry::Skill(_, _) => true,
            model::NamespaceEntry::Namespace { name, .. } => {
                let relative = name.split_whitespace().next_back().unwrap_or(name.as_str());
                !leaf_names.contains(relative)
            }
        })
        .count()
}

/// Print the global flags below the Skills section.
fn print_global_flags(col_width: usize) {
    let pad = " ".repeat(col_width.saturating_sub("--dry-run".len()));
    println!(
        "  --dry-run{}       Show rendered blocks, do not execute",
        pad
    );
    let pad2 = " ".repeat(col_width.saturating_sub("--verbose, -v".len()));
    println!(
        "  --verbose, -v{}   Show rendered blocks on stderr, then execute",
        pad2
    );
}

/// Print skill entries for a namespace drill-in (no truncation applied here;
/// caller passes `None` to disable truncation).
fn print_skill_entries(
    entries: &[model::NamespaceEntry],
    prefix: &[&str],
    _display_limit: Option<usize>,
) {
    print_skill_entries_limited(entries, prefix, usize::MAX);
}

/// Print skill entries up to `limit`, applying column alignment.
///
/// When a leaf skill and a namespace share the same relative name the namespace
/// entry is suppressed and the leaf is annotated with `[N subskills]`.
fn print_skill_entries_limited(entries: &[model::NamespaceEntry], prefix: &[&str], limit: usize) {
    use std::collections::{HashMap, HashSet};

    // When a leaf skill and a namespace share the same relative name, suppress
    // the namespace entry and annotate the leaf with "[N subskills]" instead.
    let mut namespace_map: HashMap<String, (usize, Option<String>)> = HashMap::new();
    let mut leaf_names: HashSet<String> = HashSet::new();

    for entry in entries {
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

    let mut printed = 0;
    for entry in entries {
        if printed >= limit {
            break;
        }
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
                println!("  {}{}  {desc}{suffix}", def.name.as_str().bold(), pad);
                printed += 1;
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
                    name.as_str().bold(),
                    pad,
                );
                printed += 1;
            }
        }
    }
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
