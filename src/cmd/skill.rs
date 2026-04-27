use std::io::{IsTerminal, Read};
use std::path::Path;

use yaml_rust2::YamlLoader;
use yansi::Paint;

use crate::error::CreftError;
use crate::model::AppContext;
use crate::skill_test::fixture;
use crate::wrap::{MAX_WIDTH, wrap_description};
use crate::{frontmatter, help, markdown, model, store, style, validate, yaml};

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
        let result = validate::validate_skill(&def, &blocks, &body, Some(ctx));

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
        let desc_col = 2 + max_name + 2;
        let desc_budget = MAX_WIDTH.saturating_sub(desc_col);

        for (def, source) in &flat {
            let raw_desc = format_skill_desc(def, source);
            let desc = wrap_description(&raw_desc, desc_budget, desc_col);
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
    writeln!(
        out,
        "{}creft {} <command> [ARGS] [OPTIONS]",
        "Usage: ".bold(),
        namespace,
    )
    .unwrap();
    writeln!(out).unwrap();
    writeln!(out, "{}", "Skills:".bold()).unwrap();

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
    // The flags in this section share the same description column as the skill
    // names above. Label column: 2 (indent) + col_width + 2 (gap).
    let desc_col = 2 + col_width + 2;
    let desc_budget = MAX_WIDTH.saturating_sub(desc_col);
    let mut out = String::new();

    let pad = " ".repeat(col_width.saturating_sub("--dry-run".len()));
    let dry_run_desc = wrap_description(
        "Show rendered blocks, do not execute",
        desc_budget,
        desc_col,
    );
    writeln!(out, "  --dry-run{pad}  {dry_run_desc}").unwrap();

    let pad2 = " ".repeat(col_width.saturating_sub("--verbose, -v".len()));
    let verbose_desc = wrap_description(
        "Show rendered blocks on stderr, then execute",
        desc_budget,
        desc_col,
    );
    writeln!(out, "  --verbose, -v{pad2}  {verbose_desc}").unwrap();

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
                    let label = if in_namespace {
                        relative
                    } else {
                        name.as_str()
                    };
                    Some(label.len())
                }
            }
        })
        .max()
        .unwrap_or(0);

    let desc_col = 2 + max_name + 2;
    let desc_budget = MAX_WIDTH.saturating_sub(desc_col);

    let mut out = String::new();
    let mut printed = 0;
    for entry in entries {
        if printed >= limit {
            break;
        }
        match entry {
            model::NamespaceEntry::Skill(def, source) => {
                let raw_desc = format_skill_desc(def, source);
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
                let full_desc = format!("{raw_desc}{suffix}");
                let desc = wrap_description(&full_desc, desc_budget, desc_col);
                let label = if in_namespace {
                    def.name.split_whitespace().next_back().unwrap_or(&def.name)
                } else {
                    &def.name
                };
                let pad = " ".repeat(max_name - label.len());
                writeln!(out, "  {}{}  {desc}", label.bold(), pad).unwrap();
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
                let label = if in_namespace {
                    relative
                } else {
                    name.as_str()
                };
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

/// Read a single test scenario from stdin and append (or replace) it in the
/// skill's fixture file.
///
/// The stdin envelope is YAML frontmatter (`---` delimiters) followed by the
/// scenario body. Required frontmatter fields:
///
/// - `skill`: the target skill's name (e.g. `setup`, `hooks guard bash`)
/// - `name`: the new scenario's name (must be unique within the fixture
///   unless `force` is true)
///
/// The body is the same YAML shape used by existing `*.test.yaml` entries:
/// `given/before/when/then/after/notes` keys.
///
/// The fixture path is `<local-root>/commands/<skill-path>.test.yaml`. The
/// skill must exist (its `.md` file must be resolvable in the local scope);
/// otherwise the command errors before opening the fixture.
///
/// When `force` is true and no scenario with the supplied `name` exists, the
/// function writes a stderr warning and proceeds to append. The success-message
/// branch reports "added", not "replaced", so the actual outcome is visible.
pub fn cmd_add_test(ctx: &AppContext, force: bool) -> Result<(), CreftError> {
    // Require piped stdin — the scenario body is structured YAML and cannot
    // be expressed as a command-line flag.
    if std::io::stdin().is_terminal() {
        return Err(CreftError::Setup(
            "creft add test requires piped stdin (frontmatter + scenario YAML)".into(),
        ));
    }

    cmd_add_test_with_reader(ctx, force, std::io::stdin())
}

/// Core logic for `cmd_add_test`, reading the stdin envelope from `reader`.
///
/// Separated from `cmd_add_test` so that tests can inject a `Read` impl
/// directly rather than spawning a subprocess. `cmd_add_test` performs the
/// `IsTerminal` check and then calls this function with `std::io::stdin()`.
pub(crate) fn cmd_add_test_with_reader(
    ctx: &AppContext,
    force: bool,
    mut reader: impl std::io::Read,
) -> Result<(), CreftError> {
    let mut input = String::new();
    reader.read_to_string(&mut input)?;

    // Split into frontmatter YAML and scenario body.
    let (fm_yaml, body) = frontmatter::split(&input)?;

    // Parse the frontmatter to extract `skill` and `name`.
    let fm_docs =
        YamlLoader::load_from_str(fm_yaml).map_err(|e| CreftError::Frontmatter(e.to_string()))?;
    let fm = fm_docs
        .first()
        .and_then(|d| d.as_hash())
        .ok_or_else(|| CreftError::Frontmatter("frontmatter must be a YAML mapping".into()))?;

    let skill_name = fm
        .get(&yaml_rust2::Yaml::String("skill".into()))
        .and_then(|v| v.as_str())
        .ok_or_else(|| CreftError::Setup("missing required frontmatter field 'skill'".into()))?
        .to_string();

    let scenario_name = fm
        .get(&yaml_rust2::Yaml::String("name".into()))
        .and_then(|v| v.as_str())
        .ok_or_else(|| CreftError::Setup("missing required frontmatter field 'name'".into()))?
        .to_string();

    // Verify the skill exists in the local scope.
    let skill_path = store::name_to_path_in(ctx, &skill_name, model::Scope::Local)?;
    if !skill_path.exists() {
        return Err(CreftError::CommandNotFound(skill_name.clone()));
    }

    // Compute the fixture file path.
    let fixture_path = store::skill_test_fixture_path(ctx, &skill_name, model::Scope::Local)?;

    // Assemble the candidate scenario YAML list entry for validation.
    // Quote the name if needed so the assembled string always parses cleanly.
    let quoted_name = if yaml::needs_quoting(&scenario_name) {
        let mut s = String::new();
        yaml::emit_quoted(&mut s, &scenario_name);
        s
    } else {
        scenario_name.clone()
    };

    // Indent every non-empty body line by two spaces (continuation indent
    // under the `- name: ...` list marker).
    let indented_body: String = body
        .lines()
        .map(|line| {
            if line.is_empty() {
                String::new()
            } else {
                format!("  {line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    let candidate = format!("- name: {quoted_name}\n{indented_body}\n");

    // Defensive check: the body must not repeat a `name:` key at the top level
    // because we supply it in the framing above.
    check_body_no_name_key(body)?;

    // Validate the candidate by parsing it through the typed fixture parser.
    fixture::parse_scenarios_str(&candidate, Path::new("<stdin>"))
        .map_err(|e| CreftError::Setup(format!("scenario validation failed: {e}")))?;

    // Parse the new scenario as a raw YAML node for byte-fidelity rendering.
    let new_nodes = fixture::parse_scenarios_yaml(&candidate, Path::new("<stdin>"))
        .map_err(|e| CreftError::Setup(format!("scenario validation failed: {e}")))?;
    let new_node = new_nodes.into_iter().next().ok_or_else(|| {
        CreftError::Setup("candidate produced no YAML nodes after parsing".into())
    })?;

    // Read the existing fixture (empty string if the file does not yet exist).
    let existing_content = if fixture_path.exists() {
        std::fs::read_to_string(&fixture_path)?
    } else {
        String::new()
    };

    // Parse the existing scenarios for collision detection.
    let existing_scenarios = fixture::parse_scenarios_str(&existing_content, &fixture_path)
        .map_err(|e| CreftError::Setup(format!("existing fixture is malformed: {e}")))?;

    let collision_idx = fixture::find_scenario_by_name(&existing_scenarios, &scenario_name);

    let new_content = match (collision_idx, force) {
        // Collision and --force → replace.
        (Some(idx), true) => {
            fixture::replace_scenario(&existing_content, idx, &new_node, &fixture_path)
                .map_err(|e| CreftError::Setup(format!("replace failed: {e}")))?
        }
        // Collision and no --force → error.
        (Some(_), false) => {
            return Err(CreftError::CommandAlreadyExists(format!(
                "test '{scenario_name}' already exists in {}.test.yaml; use --force to replace",
                skill_name
            )));
        }
        // No collision and --force → warn, then append.
        (None, true) => {
            eprintln!(
                "warning: --force given but no scenario named '{scenario_name}' exists in {}; appending as a new scenario",
                fixture_path.display()
            );
            let rendered = fixture::render_scenario_yaml(&new_node);
            fixture::append_scenario(&existing_content, &rendered)
        }
        // No collision and no --force → append.
        (None, false) => {
            let rendered = fixture::render_scenario_yaml(&new_node);
            fixture::append_scenario(&existing_content, &rendered)
        }
    };

    // Write the result.
    if let Some(parent) = fixture_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&fixture_path, &new_content)?;

    // Emit the appropriate success message.
    let fixture_display = fixture_path.display();
    match (collision_idx, force) {
        (Some(_), true) => {
            eprintln!(
                "replaced test: {skill_name} / {scenario_name} (in {fixture_display})\n\
                 note: --force replaces a scenario by re-emitting the file; YAML comments may not be preserved"
            );
        }
        (None, true) => {
            eprintln!(
                "added test: {skill_name} / {scenario_name} \
                 (--force matched no existing scenario; appended as new) (in {fixture_display})"
            );
        }
        _ if existing_content.is_empty() => {
            eprintln!("created: {fixture_display}\nadded test: {skill_name} / {scenario_name}");
        }
        _ => {
            eprintln!("added test: {skill_name} / {scenario_name} (in {fixture_display})");
        }
    }

    Ok(())
}

/// Return an error if the body YAML contains a top-level `name:` key.
///
/// The frontmatter supplies `name` as part of the list-entry framing; a
/// duplicate key in the body would create ambiguous YAML and a misleading
/// error from the scenario parser.
///
/// Bodies that fail YAML parsing fall through silently — `parse_scenarios_str`
/// (called by the caller immediately after) will surface any parse error with
/// a more precise diagnostic.
fn check_body_no_name_key(body: &str) -> Result<(), CreftError> {
    // Only check non-empty body — an empty body is a valid (minimal) scenario.
    if body.trim().is_empty() {
        return Ok(());
    }
    let docs = YamlLoader::load_from_str(body).unwrap_or_default();
    if docs
        .first()
        .and_then(|d| d.as_hash())
        .is_some_and(|hash| hash.contains_key(&yaml_rust2::Yaml::String("name".into())))
    {
        return Err(CreftError::Setup(
            "frontmatter 'name' conflicts with body 'name' field; remove the 'name' key from the scenario body".into(),
        ));
    }
    Ok(())
}

/// Format the description column for a skill entry.
///
/// Returns the raw description string with optional package annotation.
/// The caller is responsible for wrapping to fit the column layout.
///
/// Scope annotations (`(local)` / `(global)`) are omitted — they are noise
/// for discovery. Package provenance is preserved as `(pkg: <name>)` because
/// it is actionable (users can uninstall or inspect package skills separately).
pub fn format_skill_desc(def: &model::CommandDef, source: &model::SkillSource) -> String {
    match source {
        model::SkillSource::Owned(_) => def.description.clone(),
        model::SkillSource::Package(pkg, _) => format!("{}  (pkg: {pkg})", def.description),
        model::SkillSource::Plugin(name) => format!("{}  (plugin: {name})", def.description),
    }
}

// ── cmd_add_test tests ────────────────────────────────────────────────────────

#[cfg(test)]
mod add_test_tests {
    use pretty_assertions::assert_eq;

    use crate::error::CreftError;
    use crate::model::AppContext;
    use crate::skill_test::fixture;

    /// Build a project root with `.creft/commands/` and return `(home_tmp, project_tmp, ctx)`.
    fn project_with_commands_dir() -> (tempfile::TempDir, tempfile::TempDir, AppContext) {
        let home_tmp = tempfile::TempDir::new().expect("home tmp");
        let project_tmp = tempfile::TempDir::new().expect("project tmp");
        std::fs::create_dir_all(project_tmp.path().join(".creft/commands"))
            .expect("create commands dir");
        let ctx = AppContext::for_test(
            home_tmp.path().to_path_buf(),
            project_tmp.path().to_path_buf(),
        );
        (home_tmp, project_tmp, ctx)
    }

    /// Write a skill `.md` file so the skill "exists" for the existence check.
    fn write_skill(project: &tempfile::TempDir, skill_name: &str) {
        let parts: Vec<&str> = skill_name.split_whitespace().collect();
        let mut path = project.path().join(".creft/commands");
        for part in &parts[..parts.len().saturating_sub(1)] {
            path = path.join(part);
        }
        std::fs::create_dir_all(&path).expect("create skill namespace dirs");
        let leaf = parts.last().unwrap();
        std::fs::write(
            path.join(format!("{leaf}.md")),
            "---\nname: test\ndescription: test\n---\n```bash\necho hi\n```\n",
        )
        .expect("write skill md");
    }

    /// Write a fixture YAML file for a skill.
    fn write_fixture(project: &tempfile::TempDir, skill_name: &str, yaml: &str) {
        let path = project
            .path()
            .join(".creft/commands")
            .join(format!("{skill_name}.test.yaml"));
        std::fs::write(&path, yaml).expect("write fixture");
    }

    fn read_fixture(project: &tempfile::TempDir, skill_name: &str) -> String {
        let path = project
            .path()
            .join(".creft/commands")
            .join(format!("{skill_name}.test.yaml"));
        std::fs::read_to_string(&path).expect("read fixture")
    }

    /// A minimal valid scenario body (no `when.argv` key would fail validation,
    /// so this includes the required fields).
    const MINIMAL_BODY: &str = "when:\n  argv: [sh, -c, exit 0]\nthen:\n  exit_code: 0\n";

    /// Build a full stdin envelope for `cmd_add_test`.
    fn envelope(skill: &str, name: &str, body: &str) -> String {
        format!("---\nskill: {skill}\nname: {name}\n---\n{body}")
    }

    /// Run `cmd_add_test_with_reader` — the production handler — with the given
    /// stdin content as a `Cursor`. Tests exercise the same code path that runs
    /// in production; only the `IsTerminal` check (in `cmd_add_test`) is skipped.
    fn run(ctx: &AppContext, force: bool, input: &str) -> Result<(), CreftError> {
        super::cmd_add_test_with_reader(ctx, force, std::io::Cursor::new(input.to_owned()))
    }

    // ── Tests ─────────────────────────────────────────────────────────────────

    #[test]
    fn cmd_add_test_creates_fixture_when_absent() {
        let (_home, project, ctx) = project_with_commands_dir();
        write_skill(&project, "setup");

        let input = envelope("setup", "fresh install succeeds", MINIMAL_BODY);
        run(&ctx, false, &input).expect("should succeed");

        let content = read_fixture(&project, "setup");
        assert!(
            content.contains("fresh install succeeds"),
            "fixture must contain the scenario name; got:\n{content}"
        );
    }

    #[test]
    fn cmd_add_test_appends_to_existing_fixture() {
        let (_home, project, ctx) = project_with_commands_dir();
        write_skill(&project, "setup");
        write_fixture(
            &project,
            "setup",
            "- name: existing\n  when:\n    argv: [sh, -c, exit 0]\n  then:\n    exit_code: 0\n",
        );

        let input = envelope("setup", "new scenario", MINIMAL_BODY);
        run(&ctx, false, &input).expect("should succeed");

        let content = read_fixture(&project, "setup");
        assert!(
            content.contains("existing"),
            "existing scenario must survive the append; got:\n{content}"
        );
        assert!(
            content.contains("new scenario"),
            "new scenario must be present after append; got:\n{content}"
        );
    }

    #[test]
    fn cmd_add_test_preserves_comments_on_append() {
        let (_home, project, ctx) = project_with_commands_dir();
        write_skill(&project, "setup");
        write_fixture(
            &project,
            "setup",
            "# leading comment\n- name: existing\n  when:\n    argv: [sh, -c, exit 0]\n  then:\n    exit_code: 0\n",
        );

        let input = envelope("setup", "second", MINIMAL_BODY);
        run(&ctx, false, &input).expect("should succeed");

        let content = read_fixture(&project, "setup");
        assert!(
            content.contains("# leading comment"),
            "leading comment must survive append; got:\n{content}"
        );
    }

    #[test]
    fn cmd_add_test_collision_without_force_errors() {
        let (_home, project, ctx) = project_with_commands_dir();
        let fixture_content =
            "- name: foo\n  when:\n    argv: [sh, -c, exit 0]\n  then:\n    exit_code: 0\n";
        write_skill(&project, "setup");
        write_fixture(&project, "setup", fixture_content);

        let input = envelope("setup", "foo", MINIMAL_BODY);
        let result = run(&ctx, false, &input);
        assert!(
            matches!(result, Err(CreftError::CommandAlreadyExists(ref msg)) if msg.contains("foo")),
            "collision without --force must return CommandAlreadyExists; got: {result:?}",
        );

        // Validate before write: the fixture must be byte-for-byte unchanged.
        let after = read_fixture(&project, "setup");
        assert_eq!(
            fixture_content, after,
            "fixture must be unchanged when collision is rejected"
        );
    }

    #[test]
    fn cmd_add_test_collision_with_force_replaces() {
        let (_home, project, ctx) = project_with_commands_dir();
        write_skill(&project, "setup");
        write_fixture(
            &project,
            "setup",
            "- name: foo\n  when:\n    argv: [sh, -c, exit 1]\n  then:\n    exit_code: 1\n\
             - name: other\n  when:\n    argv: [sh, -c, exit 0]\n  then:\n    exit_code: 0\n",
        );

        let replacement_body = "when:\n  argv: [sh, -c, exit 0]\nthen:\n  exit_code: 0\n";
        let input = envelope("setup", "foo", replacement_body);
        run(&ctx, true, &input).expect("should succeed with --force");

        let content = read_fixture(&project, "setup");
        let scenarios = fixture::parse_scenarios_str(&content, std::path::Path::new("<test>"))
            .expect("fixture must parse after replace");

        assert_eq!(
            scenarios.len(),
            2,
            "both scenarios must be present after replace"
        );

        let foo = scenarios
            .iter()
            .find(|s| s.name == "foo")
            .expect("foo scenario must still exist");
        assert_eq!(
            foo.then.exit_code, 0,
            "replaced 'foo' scenario must have exit_code 0 (replacement value), not 1 (original)"
        );

        let other = scenarios.iter().find(|s| s.name == "other");
        assert!(
            other.is_some(),
            "non-colliding 'other' scenario must survive the replace"
        );
    }

    #[test]
    fn cmd_add_test_force_without_collision_warns_and_appends() {
        let (_home, project, ctx) = project_with_commands_dir();
        write_skill(&project, "setup");
        write_fixture(
            &project,
            "setup",
            "- name: foo\n  when:\n    argv: [sh, -c, exit 0]\n  then:\n    exit_code: 0\n",
        );

        let input = envelope("setup", "bar", MINIMAL_BODY);
        run(&ctx, true, &input).expect("should succeed with --force and no collision");

        let content = read_fixture(&project, "setup");
        let scenarios = fixture::parse_scenarios_str(&content, std::path::Path::new("<test>"))
            .expect("fixture must parse after --force append");

        assert_eq!(
            scenarios.len(),
            2,
            "--force without collision must append as new, resulting in two scenarios"
        );
        assert!(
            scenarios.iter().any(|s| s.name == "foo"),
            "original scenario 'foo' must survive; got names: {:?}",
            scenarios.iter().map(|s| &s.name).collect::<Vec<_>>()
        );
        assert!(
            scenarios.iter().any(|s| s.name == "bar"),
            "new scenario 'bar' must be appended; got names: {:?}",
            scenarios.iter().map(|s| &s.name).collect::<Vec<_>>()
        );
    }

    #[test]
    fn cmd_add_test_force_without_collision_emits_added_message() {
        // Scenario appended (not replaced) when --force finds no collision.
        // The fixture must have both the original and the new scenario as valid entries.
        let (_home, project, ctx) = project_with_commands_dir();
        write_skill(&project, "setup");
        write_fixture(
            &project,
            "setup",
            "- name: foo\n  when:\n    argv: [sh, -c, exit 0]\n  then:\n    exit_code: 0\n",
        );

        let input = envelope("setup", "bar", MINIMAL_BODY);
        run(&ctx, true, &input).expect("force without collision must succeed");

        let content = read_fixture(&project, "setup");
        let scenarios = fixture::parse_scenarios_str(&content, std::path::Path::new("<test>"))
            .expect("fixture after --force append must parse as valid");
        assert_eq!(
            scenarios.len(),
            2,
            "fixture must have exactly two scenarios after --force append; got {}",
            scenarios.len()
        );
        // The new scenario must be structurally valid (not just a string in the file).
        assert!(
            scenarios.iter().any(|s| s.name == "bar"),
            "appended scenario 'bar' must be present and parseable"
        );
    }

    #[test]
    fn cmd_add_test_malformed_body_rejected() {
        let (_home, project, ctx) = project_with_commands_dir();
        write_skill(&project, "setup");

        // Body is missing `when.argv` — required by the fixture schema.
        let bad_body = "then:\n  exit_code: 0\n";
        let input = envelope("setup", "broken", bad_body);
        let result = run(&ctx, false, &input);
        assert!(
            matches!(result, Err(CreftError::Setup(ref msg)) if msg.contains("scenario validation failed")),
            "malformed body must return Setup error; got: {result:?}",
        );

        // The fixture must not have been created.
        let path = project.path().join(".creft/commands/setup.test.yaml");
        assert!(
            !path.exists(),
            "fixture must not be created when body is invalid"
        );
    }

    #[test]
    fn cmd_add_test_missing_skill_rejected() {
        let (_home, _project, ctx) = project_with_commands_dir();
        // No skill file written — skill does not exist.
        let input = envelope("nonexistent", "scenario", MINIMAL_BODY);
        let result = run(&ctx, false, &input);
        assert!(
            matches!(result, Err(CreftError::CommandNotFound(_))),
            "missing skill must return CommandNotFound; got: {result:?}",
        );
    }

    #[test]
    fn cmd_add_test_missing_skill_field_rejected() {
        let (_home, _project, ctx) = project_with_commands_dir();
        // Frontmatter without `skill:`.
        let input = "---\nname: myscenario\n---\n".to_string() + MINIMAL_BODY;
        let result = run(&ctx, false, &input);
        assert!(
            matches!(result, Err(CreftError::Setup(ref msg)) if msg.contains("'skill'")),
            "missing skill field must return Setup error naming 'skill'; got: {result:?}",
        );
    }

    #[test]
    fn cmd_add_test_missing_name_field_rejected() {
        let (_home, _project, ctx) = project_with_commands_dir();
        // Frontmatter without `name:`.
        let input = "---\nskill: setup\n---\n".to_string() + MINIMAL_BODY;
        let result = run(&ctx, false, &input);
        assert!(
            matches!(result, Err(CreftError::Setup(ref msg)) if msg.contains("'name'")),
            "missing name field must return Setup error naming 'name'; got: {result:?}",
        );
    }

    #[test]
    fn cmd_add_test_namespaced_skill_resolves_correct_path() {
        let (_home, project, ctx) = project_with_commands_dir();
        write_skill(&project, "hooks guard bash");

        let input = envelope("hooks guard bash", "guards against empty", MINIMAL_BODY);
        run(&ctx, false, &input).expect("should succeed for namespaced skill");

        let path = project
            .path()
            .join(".creft/commands/hooks/guard/bash.test.yaml");
        assert!(
            path.exists(),
            "namespaced skill fixture must be written to the correct nested path"
        );
        let content = std::fs::read_to_string(&path).expect("read fixture");
        assert!(
            content.contains("guards against empty"),
            "fixture must contain the scenario name; got:\n{content}"
        );
    }

    #[test]
    fn cmd_add_test_missing_frontmatter_rejected() {
        let (_home, _project, ctx) = project_with_commands_dir();
        // No `---` delimiters at all.
        let result = run(&ctx, false, "just body text, no frontmatter\n");
        assert!(
            matches!(result, Err(CreftError::MissingFrontmatterDelimiter)),
            "missing frontmatter must return MissingFrontmatterDelimiter; got: {result:?}",
        );
    }

    #[test]
    fn check_body_no_name_key_rejects_body_with_name_field() {
        let (_home, project, ctx) = project_with_commands_dir();
        write_skill(&project, "setup");
        // Body contains a top-level `name:` key — this conflicts with the frontmatter name.
        let bad_body =
            "name: should-not-be-here\nwhen:\n  argv: [sh, -c, exit 0]\nthen:\n  exit_code: 0\n";
        let input = envelope("setup", "collider", bad_body);
        let result = run(&ctx, false, &input);
        assert!(
            matches!(result, Err(CreftError::Setup(ref msg)) if msg.contains("conflicts")),
            "body with top-level 'name' key must return a Setup error about conflict; got: {result:?}",
        );
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use serial_test::serial;

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
    #[serial]
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
    #[serial]
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
    #[serial]
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
    #[serial]
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

    /// Namespace listing orders Usage: before Skills: to match root listing structure.
    ///
    /// The root listing renders tagline → Usage → Commands/Skills → flags → footer.
    /// Namespace listing must follow the same ordering so both feel like the same UI.
    #[test]
    #[serial]
    fn namespace_listing_usage_appears_before_skills_header() {
        yansi::disable();
        let entries = synthetic_namespace_entries(2);
        let prefix: Vec<&str> = vec!["artifacts"];
        let output = render_namespace_listing(&entries, &prefix, "artifacts");
        yansi::enable();

        let usage_pos = output.find("Usage:").expect("Usage: must be present");
        let skills_pos = output.find("Skills:").expect("Skills: must be present");
        assert!(
            usage_pos < skills_pos,
            "Usage: must appear before Skills: in namespace listing;\ngot:\n{output}",
        );
    }

    /// Namespace listing incorporates the namespace name in usage and footer lines.
    ///
    /// The caller-supplied namespace name must appear in the `Usage:` line and in
    /// the `See 'creft ... --help'` footer so the user knows which namespace they
    /// are viewing.
    #[test]
    #[serial]
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
    #[serial]
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
    #[serial]
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
