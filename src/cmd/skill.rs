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
    names: bool,
    namespace: Vec<String>,
) -> Result<(), CreftError> {
    let all = store::list_all_with_source(ctx)?;

    // --names: machine-readable output for shell completion scripts.
    // One skill name per line, no ANSI, no headers, no descriptions.
    if names {
        for (def, _) in &all {
            println!("{}", def.name);
        }
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
        print!("{}", render_root_listing(&entries, compute_display_limit()));
    } else {
        // Namespace drill-in: skills in the requested namespace only, no builtins.
        if entries.is_empty() {
            eprintln!("no commands found. use 'creft add' to create one.");
            return Ok(());
        }
        let ns = prefix.join(" ");
        print!("{}", render_namespace_listing(&entries, &prefix, &ns));
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

/// Render the root listing as a `String`: tagline, usage, Commands: section, then Skills: section.
///
/// Accepting `display_limit` as a parameter makes this function unit-testable with synthetic data
/// without spawning a subprocess or touching the terminal.
pub(crate) fn render_root_listing(
    entries: &[model::NamespaceEntry],
    display_limit: Option<usize>,
) -> String {
    let mut out = String::new();
    use std::fmt::Write;

    writeln!(out, "{}", help::ROOT_ABOUT.bold()).unwrap();
    writeln!(out).unwrap();
    writeln!(out, "{}creft <command> [ARGS] [OPTIONS]", "Usage: ".bold()).unwrap();
    writeln!(out).unwrap();

    // Commands: section — fixed list of builtins, always shown in full.
    writeln!(out, "{}", "Commands:".bold()).unwrap();
    let builtins = help::builtins();
    let max_builtin_name = builtins.iter().map(|e| e.name.len()).max().unwrap_or(0);
    for entry in builtins {
        let pad = " ".repeat(max_builtin_name - entry.name.len());
        writeln!(out, "  {}{}  {}", entry.name.bold(), pad, entry.description).unwrap();
    }

    if entries.is_empty() {
        // No skills installed — omit the Skills: section entirely.
        writeln!(out).unwrap();
        out.push_str(&render_global_flags(max_builtin_name));
        writeln!(out).unwrap();
        writeln!(out, "See 'creft <command> --help' for details.").unwrap();
        return out;
    }

    // Skills: section.
    writeln!(out).unwrap();
    writeln!(out, "{}", "Skills:".bold()).unwrap();

    let empty_prefix: &[&str] = &[];
    let total = count_visible_entries(entries);
    let limit = display_limit.unwrap_or(usize::MAX);

    let (skills_output, max_skill_name) =
        render_skill_entries_limited(entries, empty_prefix, limit);
    out.push_str(&skills_output);

    if total > limit {
        writeln!(
            out,
            "  (showing {limit} of {total} — use 'creft list --all' to see all)"
        )
        .unwrap();
    }

    writeln!(out).unwrap();
    // Global flags align with the Skills: section; fall back to Commands: width if
    // no skills were rendered (defensive — the empty branch above handles the real
    // no-skills case).
    let flags_col = if max_skill_name > 0 {
        max_skill_name
    } else {
        max_builtin_name
    };
    out.push_str(&render_global_flags(flags_col));
    writeln!(out).unwrap();
    writeln!(out, "See 'creft <command> --help' for details.").unwrap();
    out
}

/// Render the namespace drill-in listing as a `String`.
///
/// Mirrors the structure of `render_root_listing` but scoped to a single namespace:
/// `Skills:` header, usage line, skill entries with column alignment, global flags,
/// and a footer pointing to per-command `--help`.
pub(crate) fn render_namespace_listing(
    entries: &[model::NamespaceEntry],
    prefix: &[&str],
    namespace: &str,
) -> String {
    use std::fmt::Write;

    let mut out = String::new();

    writeln!(out, "{}", help::ROOT_ABOUT.bold()).unwrap();
    writeln!(out).unwrap();
    writeln!(out, "{}", "Skills:".bold()).unwrap();
    writeln!(
        out,
        "{}creft {} <command> [ARGS] [OPTIONS]",
        "Usage: ".bold(),
        namespace,
    )
    .unwrap();
    writeln!(out).unwrap();

    let (skills_output, max_skill_name) = render_skill_entries_limited(entries, prefix, usize::MAX);
    out.push_str(&skills_output);

    writeln!(out).unwrap();
    out.push_str(&render_global_flags(max_skill_name));
    writeln!(out).unwrap();
    writeln!(
        out,
        "See 'creft {} <command> --help' for details.",
        namespace,
    )
    .unwrap();
    out
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

/// Render the global flags section as a `String`.
///
/// `col_width` is the column width of the section immediately above (either
/// Skills: or Commands: when no skills are installed), so that the flag labels
/// align with that section's name column.
fn render_global_flags(col_width: usize) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    let pad = " ".repeat(col_width.saturating_sub("--dry-run".len()));
    writeln!(
        out,
        "  --dry-run{}       Show rendered blocks, do not execute",
        pad
    )
    .unwrap();
    let pad2 = " ".repeat(col_width.saturating_sub("--verbose, -v".len()));
    writeln!(
        out,
        "  --verbose, -v{}   Show rendered blocks on stderr, then execute",
        pad2
    )
    .unwrap();
    out
}

/// Render skill entries up to `limit` as a `(String, max_name_width)` pair.
///
/// Returns both the rendered output and the column width used for name alignment,
/// so the caller can align subsequent sections (e.g., global flags) consistently.
///
/// When a leaf skill and a namespace share the same relative name the namespace
/// entry is suppressed and the leaf is annotated with `[N subskills]`.
fn render_skill_entries_limited(
    entries: &[model::NamespaceEntry],
    prefix: &[&str],
    limit: usize,
) -> (String, usize) {
    use std::collections::{HashMap, HashSet};
    use std::fmt::Write;

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

    // When drilling into a namespace, display only the relative portion of each
    // name (the part after the prefix). At root level the prefix is empty and
    // the full name is used. Both the column-width computation and the rendered
    // label use the same display string so they stay in sync.
    let in_namespace = !prefix.is_empty();

    // Exclude suppressed namespace entries from column-width computation.
    let max_name = entries
        .iter()
        .filter_map(|e| match e {
            model::NamespaceEntry::Skill(def, _) => {
                let label = if in_namespace {
                    def.name.split_whitespace().next_back().unwrap_or(&def.name)
                } else {
                    &def.name
                };
                Some(label.len())
            }
            model::NamespaceEntry::Namespace { name, .. } => {
                let relative = name.split_whitespace().next_back().unwrap_or(name.as_str());
                if leaf_names.contains(relative) {
                    None
                } else {
                    let label = if in_namespace { relative } else { name.as_str() };
                    Some(label.len())
                }
            }
        })
        .max()
        .unwrap_or(0);

    let mut out = String::new();
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
                let label = if in_namespace {
                    def.name.split_whitespace().next_back().unwrap_or(&def.name)
                } else {
                    &def.name
                };
                let pad = " ".repeat(max_name - label.len());
                writeln!(out, "  {}{}  {desc}{suffix}", label.bold(), pad).unwrap();
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
                let label = if in_namespace { relative } else { name.as_str() };
                let plural = if *skill_count == 1 { "skill" } else { "skills" };
                let pkg_suffix = if package.is_some() { "  [package]" } else { "" };
                let pad = " ".repeat(max_name - label.len());
                writeln!(
                    out,
                    "  {}{}  {skill_count} {plural}{pkg_suffix}",
                    label.bold(),
                    pad,
                )
                .unwrap();
                printed += 1;
            }
        }
    }
    (out, max_name)
}

/// Show a skill definition.
///
/// When `blocks` is `false`, prints the full raw markdown (frontmatter + body).
/// When `blocks` is `true`, prints only the code block contents — equivalent to
/// the old `creft show --blocks` behavior.
pub fn cmd_show(ctx: &AppContext, name: &str, blocks: bool) -> Result<(), CreftError> {
    let args: Vec<String> = name.split_whitespace().map(String::from).collect();
    let (resolved_name, _, source) = store::resolve_command(ctx, &args)?;
    if blocks {
        let cmd = store::load_from(ctx, &resolved_name, &source)?;
        for block in &cmd.blocks {
            println!("{}", block.code);
        }
    } else {
        let content = store::read_raw_from(ctx, &resolved_name, &source)?;
        println!("{}", content);
    }
    Ok(())
}

pub fn cmd_rm(ctx: &AppContext, name: &str, global: bool) -> Result<(), CreftError> {
    let args: Vec<String> = name.split_whitespace().map(String::from).collect();
    let (_, _, source) = store::resolve_command(ctx, &args)?;

    match &source {
        model::SkillSource::Package(_, _) => {
            return Err(CreftError::Setup(
                "cannot remove individual skills from an installed package -- use 'creft plugin uninstall <package>' instead".into(),
            ));
        }
        model::SkillSource::Plugin(_) => {
            return Err(CreftError::Setup(
                "cannot remove individual skills from a plugin -- use 'creft plugin deactivate <plugin>' or 'creft plugin uninstall <plugin>' instead".into(),
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

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    /// Construct a slice of `n` synthetic `NamespaceEntry::Namespace` items.
    ///
    /// Names are `"skill-0"`, `"skill-1"`, … so each entry is distinct and
    /// has a deterministic length for column-width assertions.
    fn synthetic_namespace_entries(n: usize) -> Vec<model::NamespaceEntry> {
        (0..n)
            .map(|i| model::NamespaceEntry::Namespace {
                name: format!("skill-{i}"),
                skill_count: 1,
                package: None,
            })
            .collect()
    }

    /// 100 entries with a display limit of 20 produces the truncation footer.
    ///
    /// The footer text is the user-visible message that tells them how to see the
    /// rest: `(showing 20 of 100 — use 'creft list --all' to see all)`.
    #[test]
    fn truncation_footer_shown_when_entries_exceed_limit() {
        yansi::disable();
        let entries = synthetic_namespace_entries(100);
        let output = render_root_listing(&entries, Some(20));
        yansi::enable();

        assert!(
            output.contains("(showing 20 of 100 — use 'creft list --all' to see all)"),
            "output must contain truncation footer when 100 entries exceed limit of 20;\
             \ngot:\n{output}",
        );
    }

    /// 15 entries with a display limit of 20 produces no truncation footer.
    ///
    /// When all entries fit within the limit there is nothing to truncate,
    /// so the footer must not appear.
    #[test]
    fn truncation_footer_absent_when_entries_fit_within_limit() {
        yansi::disable();
        let entries = synthetic_namespace_entries(15);
        let output = render_root_listing(&entries, Some(20));
        yansi::enable();

        assert!(
            !output.contains("(showing"),
            "output must not contain truncation footer when 15 entries fit within limit of 20;\
             \ngot:\n{output}",
        );
    }

    /// `display_limit = None` (non-TTY path) produces no truncation regardless of
    /// how many entries there are.
    ///
    /// Non-TTY output is piped or redirected; all entries are always shown.
    #[test]
    fn no_truncation_when_display_limit_is_none() {
        yansi::disable();
        let entries = synthetic_namespace_entries(200);
        let output = render_root_listing(&entries, None);
        yansi::enable();

        assert!(
            !output.contains("(showing"),
            "output must not contain truncation footer when display_limit is None;\
             \ngot:\n{output}",
        );
        // All 200 entries must appear.
        assert_eq!(
            entries
                .iter()
                .filter(|e| matches!(e, model::NamespaceEntry::Namespace { .. }))
                .count(),
            200,
            "synthetic_namespace_entries must have produced 200 entries",
        );
        assert!(
            output.contains("skill-199"),
            "all entries must be rendered when no limit is applied;\
             \ngot:\n{output}",
        );
    }

    /// Namespace listing contains Skills: header, usage line, flags, and footer.
    ///
    /// These structural elements mirror the root listing so that the namespace
    /// output feels like a natural subset rather than a bare dump.
    #[test]
    fn namespace_listing_contains_required_structural_elements() {
        yansi::disable();
        let entries = synthetic_namespace_entries(3);
        let prefix: Vec<&str> = vec!["artifacts"];
        let output = render_namespace_listing(&entries, &prefix, "artifacts");
        yansi::enable();

        assert!(
            output.contains("Skills:"),
            "namespace listing must contain 'Skills:' header;\ngot:\n{output}",
        );
        assert!(
            output.contains("Usage:"),
            "namespace listing must contain 'Usage:' line;\ngot:\n{output}",
        );
        assert!(
            output.contains("--dry-run"),
            "namespace listing must contain --dry-run flag;\ngot:\n{output}",
        );
        assert!(
            output.contains("--verbose"),
            "namespace listing must contain --verbose flag;\ngot:\n{output}",
        );
        assert!(
            output.contains("--help"),
            "namespace listing must contain --help footer;\ngot:\n{output}",
        );
    }

    /// Namespace listing incorporates the namespace name in usage and footer lines.
    ///
    /// The caller-supplied namespace name must appear in the `Usage:` line and in
    /// the `See 'creft ... --help'` footer so the user knows which namespace they
    /// are viewing.
    #[test]
    fn namespace_listing_includes_namespace_name_in_usage_and_footer() {
        yansi::disable();
        let entries = synthetic_namespace_entries(2);
        let prefix: Vec<&str> = vec!["my-ns"];
        let output = render_namespace_listing(&entries, &prefix, "my-ns");
        yansi::enable();

        assert!(
            output.contains("creft my-ns <command>"),
            "usage line must reference the namespace name;\ngot:\n{output}",
        );
        assert!(
            output.contains("creft my-ns <command> --help"),
            "footer must reference the namespace name;\ngot:\n{output}",
        );
    }

    /// Namespace listing begins with the project tagline.
    ///
    /// The tagline mirrors the root listing so the namespace output feels like a
    /// natural subset of the top-level help rather than a bare command dump.
    #[test]
    fn namespace_listing_includes_tagline() {
        yansi::disable();
        let entries = synthetic_namespace_entries(2);
        let prefix: Vec<&str> = vec!["artifacts"];
        let output = render_namespace_listing(&entries, &prefix, "artifacts");
        yansi::enable();

        assert!(
            output.contains(help::ROOT_ABOUT),
            "namespace listing must begin with the project tagline;\ngot:\n{output}",
        );
    }

    /// Skill names in namespace listings show only the relative component.
    ///
    /// When a user drills into `creft artifacts`, a skill named `artifacts cleanup`
    /// must appear as `cleanup` — the namespace prefix is implicit from context.
    #[test]
    fn namespace_listing_strips_namespace_prefix_from_skill_names() {
        yansi::disable();

        let make_skill = |name: &str| {
            model::NamespaceEntry::Skill(
                model::CommandDef {
                    name: name.to_string(),
                    description: "test skill".to_string(),
                    args: vec![],
                    flags: vec![],
                    env: vec![],
                    tags: vec![],
                    supports: vec![],
                },
                model::SkillSource::Owned(model::Scope::Local),
            )
        };

        let entries = vec![
            make_skill("artifacts cleanup"),
            make_skill("artifacts deploy"),
        ];
        let prefix: Vec<&str> = vec!["artifacts"];
        let output = render_namespace_listing(&entries, &prefix, "artifacts");
        yansi::enable();

        assert!(
            output.contains("cleanup"),
            "relative skill name must appear in namespace listing;\ngot:\n{output}",
        );
        assert!(
            output.contains("deploy"),
            "relative skill name must appear in namespace listing;\ngot:\n{output}",
        );
        assert!(
            !output.contains("artifacts cleanup"),
            "fully-qualified skill name must not appear in namespace listing;\ngot:\n{output}",
        );
        assert!(
            !output.contains("artifacts deploy"),
            "fully-qualified skill name must not appear in namespace listing;\ngot:\n{output}",
        );
    }
}
