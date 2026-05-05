use std::path::{Path, PathBuf};

use crate::error::CreftError;
use crate::frontmatter;
use crate::markdown;
pub use crate::model::walk_local_roots_from;
use crate::model::{AppContext, CommandDef, NamespaceEntry, ParsedCommand, Scope, SkillSource};
use crate::namespace::skill_namespace;
use crate::registry::{self, ActivationEntry};
use crate::search;

const RESERVED: &[&str] = &[
    "add",
    "alias",
    "completions",
    "doctor",
    "help",
    "init",
    "list",
    "plugin",
    "remove",
    "settings",
    "show",
    "skills",
    "up",
    "update",
    "version",
];

/// Returns `true` if `name` is a built-in creft subcommand that cannot be used as a skill name.
pub fn is_reserved(name: &str) -> bool {
    RESERVED.contains(&name)
}

/// Check if `.creft/` exists in the given directory (no walk-up).
///
/// Returns `Some(path)` if `<dir>/.creft` is a directory, `None` otherwise.
pub(crate) fn has_local_root(dir: &Path) -> Option<PathBuf> {
    let candidate = dir.join(".creft");
    if candidate.is_dir() {
        Some(candidate)
    } else {
        None
    }
}

/// Walk up from `start`'s parent directory collecting all `.creft/` directories.
///
/// Skips `start` itself -- only checks ancestors. Returns an empty `Vec` if no
/// ancestor has a `.creft/` directory.
pub(crate) fn walk_parent_local_roots(start: &Path) -> Vec<PathBuf> {
    let mut dir = start.to_path_buf();
    if !dir.pop() {
        return Vec::new();
    }
    walk_local_roots_from(&dir)
}

/// Convert a command name to its filesystem path within the given scope.
///
/// `"hello"` → `<scope_root>/commands/hello.md`
/// `"gh issue-body"` → `<scope_root>/commands/gh/issue-body.md`
pub fn name_to_path_in(ctx: &AppContext, name: &str, scope: Scope) -> Result<PathBuf, CreftError> {
    let parts: Vec<&str> = name.split_whitespace().collect();
    let mut path = ctx.commands_dir_for(scope)?;
    for part in &parts[..parts.len().saturating_sub(1)] {
        path = path.join(part);
    }
    if let Some(leaf) = parts.last() {
        path = path.join(format!("{}.md", leaf));
    }
    Ok(path)
}

/// Compute the fixture file path for a skill in a given scope.
///
/// `<scope-root>/commands/<path>/<basename>.test.yaml` — the same path
/// structure as the skill's `.md` file with a `.test.yaml` extension instead.
///
/// Returns `Err` for invalid or reserved skill names, matching the validation
/// that all other skill-write operations perform. Does NOT check that the path
/// exists or that the skill exists; callers validate skill existence
/// separately, before writing the fixture.
#[allow(dead_code)] // called by cmd_add_test in cmd/skill.rs
pub fn skill_test_fixture_path(
    ctx: &AppContext,
    name: &str,
    scope: Scope,
) -> Result<PathBuf, CreftError> {
    validate_name(name)?;
    let md_path = name_to_path_in(ctx, name, scope)?;
    // Replace the ".md" extension with ".test.yaml".
    let stem = md_path
        .file_stem()
        .expect("name_to_path_in always produces a path with a filename");
    let parent = md_path
        .parent()
        .expect("name_to_path_in always produces a path with a parent");
    Ok(parent.join(format!("{}.test.yaml", stem.to_string_lossy())))
}

/// Validate that a path token is safe to join into a filesystem path.
///
/// Rejects tokens containing path traversal components:
/// - `.` or `..` (directory traversal)
/// - `/` or `\` (path separators)
/// - Empty strings
///
/// Applied to tokens from CLI args before they are used in path
/// construction for package skill resolution.
pub(crate) fn validate_path_token(token: &str) -> Result<(), CreftError> {
    if token.is_empty() {
        return Err(CreftError::InvalidName("path token cannot be empty".into()));
    }
    if token == "." || token == ".." {
        return Err(CreftError::InvalidName(format!(
            "invalid path component '{}'",
            token,
        )));
    }
    if token.contains('/') || token.contains('\\') {
        return Err(CreftError::InvalidName(format!(
            "invalid characters in '{}'",
            token,
        )));
    }
    Ok(())
}

/// Validate a command name.
pub(crate) fn validate_name(name: &str) -> Result<(), CreftError> {
    if name.is_empty() {
        return Err(CreftError::InvalidName("name cannot be empty".into()));
    }

    let parts: Vec<&str> = name.split_whitespace().collect();

    if let Some(first) = parts.first()
        && is_reserved(first)
    {
        return Err(CreftError::ReservedName(first.to_string()));
    }

    for part in &parts {
        if part.is_empty() {
            return Err(CreftError::InvalidName("name parts cannot be empty".into()));
        }
        if !part
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(CreftError::InvalidName(format!(
                "invalid characters in '{}'",
                part
            )));
        }
    }

    Ok(())
}

/// Save a command definition to the store in the given scope.
pub fn save(
    ctx: &AppContext,
    content: &str,
    overwrite: bool,
    scope: Scope,
) -> Result<String, CreftError> {
    let (def, body) = frontmatter::parse(content)?;
    validate_name(&def.name)?;

    let path = name_to_path_in(ctx, &def.name, scope)?;

    if path.exists() && !overwrite {
        return Err(CreftError::CommandAlreadyExists(def.name.clone()));
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let output = frontmatter::serialize(&def, &body)?;
    std::fs::write(&path, output)?;

    // Rebuild the namespace index so search reflects the new skill immediately.
    // A failed rebuild does not prevent the save from succeeding.
    let ns = skill_namespace(&def.name).to_owned();
    if let Err(e) = search::store::rebuild_namespace_index(ctx, &ns, scope) {
        eprintln!(
            "warning: could not rebuild search index for '{}': {}",
            ns, e
        );
    }

    Ok(def.name)
}

/// Compute the path to an owned skill file inside a specific local root.
///
/// Mirrors `name_to_path_in` but operates on an explicit root directory
/// rather than resolving through `ctx`. Used by `read_raw_from` and
/// `load_from` when the owning root is already known from the `SkillSource`.
fn path_in_root(root: &Path, name: &str) -> PathBuf {
    let parts: Vec<&str> = name.split_whitespace().collect();
    let mut path = root.join("commands");
    for part in &parts[..parts.len().saturating_sub(1)] {
        path = path.join(part);
    }
    if let Some(leaf) = parts.last() {
        path = path.join(format!("{}.md", leaf));
    }
    path
}

/// Compute the path to a package skill file inside a specific local root.
///
/// `full_name` is the fully-qualified skill name including the package prefix,
/// e.g. `"mypkg deploy rollback"`. The package directory is `<root>/packages/<pkg>/`,
/// and the relative skill path mirrors the token structure.
fn package_skill_path_in_root(root: &Path, full_name: &str) -> Result<PathBuf, CreftError> {
    let tokens: Vec<&str> = full_name.split_whitespace().collect();
    if tokens.len() < 2 {
        return Err(CreftError::PackageNotFound(full_name.to_string()));
    }
    let pkg_name = tokens[0];
    let rel_parts = &tokens[1..];
    for part in rel_parts {
        validate_path_token(part)?;
    }
    let mut path = root.join("packages").join(pkg_name);
    for (i, part) in rel_parts.iter().enumerate() {
        if i == rel_parts.len() - 1 {
            path = path.join(format!("{}.md", part));
        } else {
            path = path.join(part);
        }
    }
    Ok(path)
}

/// Load and parse a skill from a file path, replacing the frontmatter name
/// with `name` (to give the caller a consistent fully-qualified identifier).
fn load_from_path(path: &Path, name: &str) -> Result<ParsedCommand, CreftError> {
    if !path.exists() {
        return Err(CreftError::CommandNotFound(name.to_string()));
    }
    let content = std::fs::read_to_string(path)?;
    let (def, body) = frontmatter::parse(&content)?;
    let (docs, blocks) = markdown::extract_blocks(&body);
    Ok(ParsedCommand { def, docs, blocks })
}

/// Load and parse a package skill from a file path.
///
/// Replaces the frontmatter name with `full_name` so callers get the
/// fully-qualified namespaced identifier.
fn load_package_skill_from_path(path: &Path, full_name: &str) -> Result<ParsedCommand, CreftError> {
    let content = std::fs::read_to_string(path).map_err(CreftError::Io)?;
    let (mut def, body) = frontmatter::parse(&content)?;
    let (docs, blocks) = markdown::extract_blocks(&body);
    def.name = full_name.to_string();
    Ok(ParsedCommand { def, docs, blocks })
}

/// Return a clone of `ctx` whose `local_roots` is replaced by the single supplied root.
///
/// Used by `resolve_command`, `list_all_with_source`, the indexer's per-root grouping,
/// and `cmd_rm` to run a per-scope helper against one local root from the chain without
/// changing those helpers' signatures. Path-derivation helpers route through
/// `resolve_root(Scope::Local)` → `nearest_local_root()`, which on a pinned context
/// returns the single supplied root.
///
/// Callees must treat the pinned context as scope-narrowed to that root. They must
/// not iterate the chain via `local_roots()` or `iter_local_roots()` — pinning
/// communicates "operate on this one root," not "the chain has shrunk."
pub(crate) fn pin_ctx_to_root(ctx: &AppContext, root: &Path) -> AppContext {
    let mut pinned = ctx.clone();
    pinned.local_roots = vec![root.to_path_buf()];
    pinned
}

/// Build an owned `SkillSource` for the given scope, reading the owning root
/// from `ctx.nearest_local_root()` for `Scope::Local`. On a pinned context
/// that root is the pinned root.
fn make_owned_source(scope: Scope, ctx: &AppContext) -> SkillSource {
    match scope {
        Scope::Local => SkillSource::owned_local(
            ctx.nearest_local_root()
                .expect("resolve_in_scope called with Local scope requires a local root")
                .to_path_buf(),
        ),
        Scope::Global => SkillSource::owned_global(),
    }
}

/// Build a package `SkillSource` for the given scope, reading the owning root
/// from `ctx.nearest_local_root()` for `Scope::Local`.
fn make_package_source(name: String, scope: Scope, ctx: &AppContext) -> SkillSource {
    match scope {
        Scope::Local => SkillSource::package_local(
            name,
            ctx.nearest_local_root()
                .expect("resolve_in_scope called with Local scope requires a local root")
                .to_path_buf(),
        ),
        Scope::Global => SkillSource::package_global(name),
    }
}

/// Load and parse a command by name from the given scope.
pub fn load_in(ctx: &AppContext, name: &str, scope: Scope) -> Result<ParsedCommand, CreftError> {
    let path = name_to_path_in(ctx, name, scope)?;
    if !path.exists() {
        return Err(CreftError::CommandNotFound(name.to_string()));
    }

    let content = std::fs::read_to_string(&path)?;
    let (def, body) = frontmatter::parse(&content)?;
    let (docs, blocks) = markdown::extract_blocks(&body);

    Ok(ParsedCommand { def, docs, blocks })
}

/// List all commands in the given scope.
pub fn list_all_in(ctx: &AppContext, scope: Scope) -> Result<Vec<CommandDef>, CreftError> {
    let base = ctx.commands_dir_for(scope)?;
    if !base.exists() {
        return Ok(Vec::new());
    }

    let mut defs = Vec::new();
    collect_commands(&base, &mut defs)?;
    defs.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(defs)
}

fn collect_commands(dir: &Path, defs: &mut Vec<CommandDef>) -> Result<(), CreftError> {
    let entries = std::fs::read_dir(dir)?;
    for entry in entries {
        let entry = entry?;
        let file_type = entry.file_type()?;

        // Skip symlinks -- prevents traversal outside the commands directory.
        if file_type.is_symlink() {
            continue;
        }

        let path = entry.path();
        if file_type.is_dir() {
            collect_commands(&path, defs)?;
        } else if path.extension().is_some_and(|e| e == "md") {
            match std::fs::read_to_string(&path) {
                Ok(content) => match frontmatter::parse(&content) {
                    Ok((def, _)) => defs.push(def),
                    Err(CreftError::MissingFrontmatterDelimiter) => continue,
                    Err(e) => eprintln!("warning: skipping {}: {}", path.display(), e),
                },
                Err(e) => eprintln!("warning: could not read {}: {}", path.display(), e),
            }
        }
    }
    Ok(())
}

/// Group a flat skill list into namespace entries for a given prefix depth.
///
/// `prefix` is the namespace path the user has drilled into. An empty slice
/// means top-level. `["aws"]` means we are inside the `aws` namespace.
///
/// Returns entries sorted alphabetically: namespaces first, then skills.
pub fn group_by_namespace(
    skills: Vec<(CommandDef, SkillSource)>,
    prefix: &[&str],
) -> Vec<NamespaceEntry> {
    use std::collections::BTreeMap;

    let mut sub_namespaces: BTreeMap<String, Vec<(CommandDef, SkillSource)>> = BTreeMap::new();
    let mut leaf_skills: Vec<(CommandDef, SkillSource)> = Vec::new();

    for (def, source) in skills {
        let parts = def.name_parts();

        if parts.len() <= prefix.len() {
            continue;
        }
        if !parts[..prefix.len()]
            .iter()
            .zip(prefix.iter())
            .all(|(a, b)| a == b)
        {
            continue;
        }

        let remaining = &parts[prefix.len()..];
        if remaining.len() == 1 {
            leaf_skills.push((def, source));
        } else {
            let ns_name = remaining[0].to_string();
            sub_namespaces
                .entry(ns_name)
                .or_default()
                .push((def, source));
        }
    }

    let mut result: Vec<NamespaceEntry> = Vec::new();

    for (ns_token, ns_skills) in sub_namespaces {
        let skill_count = ns_skills.len();

        let full_ns_name = if prefix.is_empty() {
            ns_token.clone()
        } else {
            format!("{} {}", prefix.join(" "), ns_token)
        };

        let package = detect_single_package(&ns_skills);

        result.push(NamespaceEntry::Namespace {
            name: full_ns_name,
            skill_count,
            package,
        });
    }

    leaf_skills.sort_by(|a, b| a.0.name.cmp(&b.0.name));
    for (def, source) in leaf_skills {
        result.push(NamespaceEntry::Skill(def, source));
    }

    result
}

/// Determine whether all skills in a slice come from the same package.
///
/// Returns `Some(pkg_name)` if every skill has `SkillSource::Package(pkg_name, _)` and
/// they all share the same package name. Returns `None` otherwise (mixed, owned, or empty).
fn detect_single_package(skills: &[(CommandDef, SkillSource)]) -> Option<String> {
    if skills.is_empty() {
        return None;
    }
    let mut pkg_name: Option<&str> = None;
    for (_, source) in skills {
        match source {
            SkillSource::Package { name, .. } => {
                match pkg_name {
                    None => pkg_name = Some(name.as_str()),
                    Some(existing) => {
                        if existing != name.as_str() {
                            // Multiple packages -- mixed.
                            return None;
                        }
                    }
                }
            }
            SkillSource::Owned { .. } | SkillSource::Plugin(_) => {
                // Contains an owned or plugin skill -- not a pure package namespace.
                return None;
            }
        }
    }
    pkg_name.map(|s| s.to_string())
}

/// List all skills under a namespace prefix, across all scopes.
///
/// Delegates to `list_all_with_source()` and filters by prefix.
pub fn list_namespace_skills(
    ctx: &AppContext,
    prefix: &[&str],
) -> Result<Vec<(CommandDef, SkillSource)>, CreftError> {
    let all = list_all_with_source(ctx)?;
    Ok(all
        .into_iter()
        .filter(|(def, _)| {
            let parts = def.name_parts();
            parts.len() > prefix.len()
                && parts[..prefix.len()]
                    .iter()
                    .zip(prefix.iter())
                    .all(|(a, b)| a == b)
        })
        .collect())
}

/// Check if a given namespace prefix has any skills under it.
///
/// Returns true if any skill's name starts with the given prefix tokens
/// followed by at least one more token.
pub fn namespace_exists(ctx: &AppContext, prefix: &[&str]) -> Result<bool, CreftError> {
    let all = list_all_with_source(ctx)?;
    Ok(all.into_iter().any(|(def, _)| {
        let parts = def.name_parts();
        parts.len() > prefix.len()
            && parts[..prefix.len()]
                .iter()
                .zip(prefix.iter())
                .all(|(a, b)| a == b)
    }))
}

/// Check if a given namespace prefix has any skills under it in a single scope.
///
/// Like `namespace_exists` but limited to one scope. Used by `cmd_alias_add`
/// to decide which scope file to write the alias into when the target is a
/// namespace prefix rather than a leaf skill.
pub(crate) fn namespace_exists_in_scope(
    ctx: &AppContext,
    prefix: &[&str],
    scope: Scope,
) -> Result<bool, CreftError> {
    let scope_skills = list_scope_with_packages(ctx, scope)?;
    if scope_skills.iter().any(|(def, _)| {
        let parts = def.name_parts();
        parts.len() > prefix.len()
            && parts[..prefix.len()]
                .iter()
                .zip(prefix.iter())
                .all(|(a, b)| a == b)
    }) {
        return Ok(true);
    }
    let mut plugin_skills: Vec<(CommandDef, SkillSource)> = Vec::new();
    append_activated_plugin_skills_for_scope(ctx, scope, &mut plugin_skills)?;
    Ok(plugin_skills.iter().any(|(def, _)| {
        let parts = def.name_parts();
        parts.len() > prefix.len()
            && parts[..prefix.len()]
                .iter()
                .zip(prefix.iter())
                .all(|(a, b)| a == b)
    }))
}

/// Check whether a command name is also a namespace prefix with child commands.
///
/// Returns `true` if any command exists whose name starts with `name` followed
/// by a space. For example, `has_subcommands(ctx, "test")` returns `true` if
/// `test mutants` or `test integration` exist.
///
/// Returns `false` if no child commands exist, or if only the command itself exists.
pub fn has_subcommands(ctx: &AppContext, name: &str) -> Result<bool, CreftError> {
    let parts: Vec<&str> = name.split_whitespace().collect();
    namespace_exists(ctx, &parts)
}

/// List direct child commands under a given command name prefix.
///
/// Returns `(CommandDef, SkillSource)` pairs for commands that are
/// one level deeper than `name`. For example, given `name = "test"`,
/// returns entries for `test mutants`, `test integration`, etc. --
/// but NOT `test mutants filter` (that would be a grandchild).
///
/// If deeper nesting exists (e.g., `test mutants filter`), those
/// commands are NOT included -- only the immediate next level.
pub fn list_direct_subcommands(
    ctx: &AppContext,
    name: &str,
) -> Result<Vec<(CommandDef, SkillSource)>, CreftError> {
    let parts: Vec<&str> = name.split_whitespace().collect();
    let all = list_namespace_skills(ctx, &parts)?;
    Ok(all
        .into_iter()
        .filter(|(def, _)| def.name_parts().len() == parts.len() + 1)
        .collect())
}

/// Delete a command by name from the given scope.
pub fn remove_in(ctx: &AppContext, name: &str, scope: Scope) -> Result<(), CreftError> {
    let path = name_to_path_in(ctx, name, scope)?;
    if !path.exists() {
        return Err(CreftError::CommandNotFound(name.to_string()));
    }
    std::fs::remove_file(&path)?;

    // Walk up and remove namespace subdirectories that are now empty.
    if let Some(parent) = path.parent() {
        let base = ctx.commands_dir_for(scope)?;
        let mut dir = parent.to_path_buf();
        while dir != base {
            if std::fs::read_dir(&dir)
                .map(|mut d| d.next().is_none())
                .unwrap_or(false)
            {
                let _ = std::fs::remove_dir(&dir);
                dir = match dir.parent() {
                    Some(p) => p.to_path_buf(),
                    None => break,
                };
            } else {
                break;
            }
        }
    }

    // Rebuild the namespace index without the removed skill.
    // A failed rebuild does not prevent the removal from succeeding.
    let ns = skill_namespace(name).to_owned();
    if let Err(e) = search::store::rebuild_namespace_index(ctx, &ns, scope) {
        eprintln!(
            "warning: could not rebuild search index for '{}': {}",
            ns, e
        );
    }

    Ok(())
}

/// Get the raw content of a command file from the given scope.
pub fn read_raw_in(ctx: &AppContext, name: &str, scope: Scope) -> Result<String, CreftError> {
    let path = name_to_path_in(ctx, name, scope)?;
    if !path.exists() {
        return Err(CreftError::CommandNotFound(name.to_string()));
    }
    Ok(std::fs::read_to_string(&path)?)
}

/// Get the raw content of a command file, from either an owned skill or an installed package.
///
/// For local-scoped sources, reads directly from the owning root carried on `source`
/// (no chain walk — the resolver already recorded the authoritative root).
/// For global-scoped sources and plugins, delegates to the per-scope helpers.
pub fn read_raw_from(
    ctx: &AppContext,
    name: &str,
    source: &SkillSource,
) -> Result<String, CreftError> {
    match source {
        SkillSource::Owned {
            scope: Scope::Local,
            ..
        } => {
            // local_root() is guaranteed Some(_) for local-tagged variants by constructor invariant.
            let root = source.local_root().expect("local Owned source has root");
            let path = path_in_root(root, name);
            Ok(std::fs::read_to_string(&path)?)
        }
        SkillSource::Owned { scope, .. } => read_raw_in(ctx, name, *scope),
        SkillSource::Package {
            scope: Scope::Local,
            ..
        } => {
            // Read the file directly rather than parse-and-reserialize, which would
            // drop code block contents. Use the owning root from the source.
            let root = source.local_root().expect("local Package source has root");
            let file_path = package_skill_path_in_root(root, name)?;
            Ok(std::fs::read_to_string(&file_path)?)
        }
        SkillSource::Package { .. } => {
            // Global package: fall through to the registry helper.
            let file_path = registry::skill_file_path(ctx, name)?;
            Ok(std::fs::read_to_string(&file_path)?)
        }
        SkillSource::Plugin(plugin_name) => {
            let file_path = registry::plugin_skill_file_path(ctx, plugin_name, name)?;
            Ok(std::fs::read_to_string(&file_path)?)
        }
    }
}

/// Load and parse a command by name and source.
///
/// For local-scoped sources, reads directly from the owning root carried on `source`
/// (no chain walk — the resolver already recorded the authoritative root).
/// For global-scoped sources and plugins, delegates to the per-scope helpers.
pub fn load_from(
    ctx: &AppContext,
    name: &str,
    source: &SkillSource,
) -> Result<ParsedCommand, CreftError> {
    match source {
        SkillSource::Owned {
            scope: Scope::Local,
            ..
        } => {
            let root = source.local_root().expect("local Owned source has root");
            let path = path_in_root(root, name);
            load_from_path(&path, name)
        }
        SkillSource::Owned { scope, .. } => load_in(ctx, name, *scope),
        SkillSource::Package {
            scope: Scope::Local,
            ..
        } => {
            // Use the owning root from the source directly.
            let root = source.local_root().expect("local Package source has root");
            let file_path = package_skill_path_in_root(root, name)?;
            load_package_skill_from_path(&file_path, name)
        }
        SkillSource::Package { .. } => registry::load_package_skill(ctx, name),
        SkillSource::Plugin(plugin_name) => registry::load_plugin_skill(ctx, plugin_name, name),
    }
}

/// List all skills in a single scope along with their source, including package skills.
///
/// Skills in `scope`'s commands directory are returned first, followed by package skills
/// from that scope's packages directory.
pub(crate) fn list_scope_with_packages(
    ctx: &AppContext,
    scope: Scope,
) -> Result<Vec<(CommandDef, SkillSource)>, CreftError> {
    let owned = list_all_in(ctx, scope)?;
    let owned_names: std::collections::HashSet<String> =
        owned.iter().map(|d| d.name.clone()).collect();

    // For local scope, the owning root is the nearest local root on ctx. Under chain
    // pinning (called from list_all_with_source), nearest_local_root() returns the
    // pinned root, so each emitted source carries the correct owning root.
    let local_root_buf: Option<PathBuf> = if scope == Scope::Local {
        ctx.nearest_local_root().map(|p| p.to_path_buf())
    } else {
        None
    };

    let mut result: Vec<(CommandDef, SkillSource)> = owned
        .into_iter()
        .map(|d| {
            let source = match &local_root_buf {
                Some(root) => SkillSource::owned_local(root.clone()),
                None => SkillSource::owned_global(),
            };
            (d, source)
        })
        .collect();

    let packages = registry::list_packages_in(ctx, scope)?;
    for pkg in packages {
        match registry::list_package_skills_in(ctx, &pkg.manifest.name, scope) {
            Ok(skills) => {
                for skill in skills {
                    if !owned_names.contains(&skill.name) {
                        let source = match &local_root_buf {
                            Some(root) => {
                                SkillSource::package_local(pkg.manifest.name.clone(), root.clone())
                            }
                            None => SkillSource::package_global(pkg.manifest.name.clone()),
                        };
                        result.push((skill, source));
                    }
                }
            }
            Err(e) => eprintln!(
                "warning: could not list skills for package '{}': {}",
                pkg.manifest.name, e
            ),
        }
    }

    Ok(result)
}

/// List all available skills (owned + installed + activated plugins), sorted by name.
///
/// When `creft_home` is set, lists only from that single location.
/// Otherwise, iterates every local root nearest-first, then global, then activated
/// plugins. The first occurrence of a given skill name wins (nearest-root-wins for
/// local entries; local shadows global).
///
/// Each returned `SkillSource` carries the owning local root for local-scoped skills.
pub fn list_all_with_source(
    ctx: &AppContext,
) -> Result<Vec<(CommandDef, SkillSource)>, CreftError> {
    if ctx.creft_home.is_some() {
        let mut result = list_scope_with_packages(ctx, Scope::Global)?;
        append_activated_plugin_skills(ctx, &mut result)?;
        result.sort_by(|a, b| a.0.name.cmp(&b.0.name));
        return Ok(result);
    }

    let mut result = Vec::new();
    let mut seen_names: std::collections::HashSet<String> = std::collections::HashSet::new();

    // Walk every local root nearest-first. Pin the context to each root so that
    // list_scope_with_packages reads the right directory and emits sources with the
    // correct owning root.
    for local_root in ctx.local_roots() {
        let pinned = pin_ctx_to_root(ctx, local_root);
        for item in list_scope_with_packages(&pinned, Scope::Local)? {
            if !seen_names.contains(&item.0.name) {
                seen_names.insert(item.0.name.clone());
                result.push(item);
            }
        }
    }

    for item in list_scope_with_packages(ctx, Scope::Global)? {
        if !seen_names.contains(&item.0.name) {
            seen_names.insert(item.0.name.clone());
            result.push(item);
        }
    }

    // Append activated plugin skills (all local roots + global activations merged).
    let mut plugin_items = Vec::new();
    append_activated_plugin_skills(ctx, &mut plugin_items)?;
    for item in plugin_items {
        if !seen_names.contains(&item.0.name) {
            result.push(item);
        }
    }

    result.sort_by(|a, b| a.0.name.cmp(&b.0.name));
    Ok(result)
}

/// Collect activated plugin skills for a single scope and append them to `result`.
///
/// Deduplication (via `seen_plugin_skills`) is the caller's responsibility when
/// spanning multiple scopes. When called from `append_activated_plugin_skills`,
/// the cross-scope dedup set is threaded through both calls.
pub(crate) fn append_activated_plugin_skills_for_scope(
    ctx: &AppContext,
    scope: Scope,
    result: &mut Vec<(CommandDef, SkillSource)>,
) -> Result<(), CreftError> {
    let mut seen_plugin_skills: std::collections::HashSet<String> =
        std::collections::HashSet::new();
    append_activated_plugin_skills_for_scope_dedup(ctx, scope, result, &mut seen_plugin_skills)
}

/// Inner helper that accepts a shared dedup set.
///
/// Shared between the per-scope public helper and the cross-scope
/// `append_activated_plugin_skills`, which threads one dedup set across both
/// scope calls so that a plugin activated in both local and global does not
/// produce duplicate entries.
fn append_activated_plugin_skills_for_scope_dedup(
    ctx: &AppContext,
    scope: Scope,
    result: &mut Vec<(CommandDef, SkillSource)>,
    seen_plugin_skills: &mut std::collections::HashSet<String>,
) -> Result<(), CreftError> {
    let settings = registry::load_settings(ctx, scope)?;
    for (plugin_name, entry) in &settings.activated {
        match registry::list_plugin_skills_unprefixed(ctx, plugin_name) {
            Ok(skills) => {
                for skill in skills {
                    let key = format!("{}/{}", plugin_name, skill.name);
                    if seen_plugin_skills.contains(&key) {
                        continue;
                    }
                    let activated = match entry {
                        ActivationEntry::All(true) => true,
                        ActivationEntry::Commands(cmds) => cmds.contains(&skill.name),
                        ActivationEntry::All(false) => false,
                    };
                    if activated {
                        seen_plugin_skills.insert(key);
                        result.push((skill, SkillSource::Plugin(plugin_name.clone())));
                    }
                }
            }
            Err(e) => {
                eprintln!(
                    "warning: could not list skills for plugin '{}': {}",
                    plugin_name, e
                );
            }
        }
    }
    Ok(())
}

/// Collect skills from all activated plugins and append them to `result`.
///
/// Reads activation settings from every local root nearest-first, then global.
/// A plugin command activated in any local root takes precedence over the same
/// command from global (or a farther local root) — the shared dedup set prevents
/// duplicate entries.
fn append_activated_plugin_skills(
    ctx: &AppContext,
    result: &mut Vec<(CommandDef, SkillSource)>,
) -> Result<(), CreftError> {
    let mut seen_plugin_skills: std::collections::HashSet<String> =
        std::collections::HashSet::new();

    if ctx.creft_home.is_some() {
        return append_activated_plugin_skills_for_scope_dedup(
            ctx,
            Scope::Global,
            result,
            &mut seen_plugin_skills,
        );
    }

    // Walk every local root nearest-first; pin the context so load_settings reads
    // the right settings.json for each root.
    for local_root in ctx.local_roots() {
        let pinned = pin_ctx_to_root(ctx, local_root);
        append_activated_plugin_skills_for_scope_dedup(
            &pinned,
            Scope::Local,
            result,
            &mut seen_plugin_skills,
        )?;
    }

    append_activated_plugin_skills_for_scope_dedup(
        ctx,
        Scope::Global,
        result,
        &mut seen_plugin_skills,
    )
}

/// Construct the flat-file path for a namespaced command name.
///
/// `"test mutants"` → `<scope_root>/commands/test mutants.md`
///
/// The path an LLM or human might create when using spaces in the
/// filename instead of the directory structure.
fn flat_file_path_in(ctx: &AppContext, name: &str, scope: Scope) -> Result<PathBuf, CreftError> {
    let dir = ctx.commands_dir_for(scope)?;
    Ok(dir.join(format!("{name}.md")))
}

/// Migrate a flat file with spaces to the correct directory structure.
///
/// Moves `commands/test mutants.md` → `commands/test/mutants.md`,
/// creating intermediate directories as needed.
///
/// Returns `Ok(true)` if migration occurred, `Ok(false)` if skipped
/// (target already exists or flat file not found), `Err` on IO failure.
///
/// Logs to stderr: `migrated: "test mutants.md" → test/mutants.md`
fn migrate_flat_file(ctx: &AppContext, name: &str, scope: Scope) -> Result<bool, CreftError> {
    let flat_path = flat_file_path_in(ctx, name, scope)?;

    // If the flat file doesn't exist, nothing to migrate.
    if !flat_path.exists() {
        return Ok(false);
    }

    let target_path = name_to_path_in(ctx, name, scope)?;

    // If the directory-structured target already exists, it wins.
    if target_path.exists() {
        let flat_name = flat_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| format!("{name}.md"));
        let dir_name = target_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| format!("{}.md", name.replace(' ', "/")));
        eprintln!(
            "note: \"{flat_name}\" exists but \"{dir_name}\" takes priority; flat file ignored"
        );
        return Ok(false);
    }

    // Create parent directories for the target.
    if let Some(parent) = target_path.parent() {
        std::fs::create_dir_all(parent).map_err(CreftError::Io)?;
    }

    // Atomically move the flat file to the directory structure.
    match std::fs::rename(&flat_path, &target_path) {
        Ok(()) => {
            // Build display names for the log message.
            let flat_name = flat_path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| format!("{name}.md"));
            // Convert "a b c" → "a/b/c.md" for the display path.
            let parts: Vec<&str> = name.split_whitespace().collect();
            let dir_display = format!(
                "{}/{}.md",
                parts[..parts.len() - 1].join("/"),
                parts.last().unwrap_or(&name)
            );
            eprintln!("migrated: \"{flat_name}\" → {dir_display}");
            Ok(true)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // Race condition: another process already migrated the flat file.
            // If the target now exists, the migration is done — treat as success.
            if target_path.exists() {
                Ok(true)
            } else {
                Err(CreftError::Io(e))
            }
        }
        Err(e) => Err(CreftError::Io(e)),
    }
}

/// Resolve a command within a single scope (owned commands, packages, and plugins).
///
/// Returns `(command_name, remaining_args, SkillSource)` or `CreftError::CommandNotFound`.
///
/// Exposed as `pub(crate)` so `cmd::alias` can determine which scope a target
/// lives in without re-implementing the resolution logic. The internal caller
/// (`resolve_command`) uses this function unchanged.
pub(crate) fn resolve_in_scope(
    ctx: &AppContext,
    args: &[String],
    scope: Scope,
) -> Result<(String, Vec<String>, SkillSource), CreftError> {
    let first = &args[0];

    // Try longest owned-command match first: "gh issue-body" before "gh".
    for len in (1..=args.len()).rev() {
        let candidate = args[..len].join(" ");
        let path = name_to_path_in(ctx, &candidate, scope)?;
        if path.exists() {
            // Warn when a stale flat file coexists with the directory-structured version
            // so the user knows why the flat file is being ignored. `creft doctor` also
            // surfaces this, but a point-of-use note is more actionable.
            if len >= 2 {
                let flat_path = flat_file_path_in(ctx, &candidate, scope)?;
                if flat_path.exists() {
                    let flat_name = flat_path
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| format!("{candidate}.md"));
                    eprintln!("note: \"{flat_name}\" ignored — directory version takes priority");
                }
            }
            let source = make_owned_source(scope, ctx);
            return Ok((candidate, args[len..].to_vec(), source));
        }
    }

    // Check for flat files with spaces and migrate them.
    // Start at len=2: single-token args (len=1) are individual CLI tokens and
    // cannot contain spaces, so they can never match a space-delimited flat file.
    for len in (2..=args.len()).rev() {
        let candidate = args[..len].join(" ");
        // candidate always has spaces at len>=2, so contains(' ') is always true —
        // kept as a defensive guard.
        if candidate.contains(' ') && migrate_flat_file(ctx, &candidate, scope)? {
            let path = name_to_path_in(ctx, &candidate, scope)?;
            if path.exists() {
                let source = make_owned_source(scope, ctx);
                return Ok((candidate, args[len..].to_vec(), source));
            }
        }
    }

    // Check if first arg is a namespace directory.
    let ns_dir = ctx.commands_dir_for(scope)?.join(first);
    if ns_dir.is_dir() {
        if args.len() == 1 {
            return Err(CreftError::CommandNotFound(format!(
                "'{}' is a namespace. Available commands:",
                first
            )));
        }
        return Err(CreftError::CommandNotFound(args[..2].join(" ")));
    }

    // Check if first arg matches an installed package. Try longest skill match
    // (up to 3 tokens within the package namespace, 4 total including the package name).
    let pkg_dir = ctx.packages_dir_for(scope)?.join(first);
    if pkg_dir.is_dir() {
        let remaining = &args[1..];
        for skill_len in (1..=remaining.len().min(3)).rev() {
            let skill_tokens = &remaining[..skill_len];
            // Validate tokens before constructing any path. On failure, bail immediately
            // rather than trying shorter matches — the caller sent a bad token.
            for token in skill_tokens {
                validate_path_token(token)?;
            }
            let full_name = format!("{} {}", first, skill_tokens.join(" "));
            let mut file_path = pkg_dir.clone();
            for (i, token) in skill_tokens.iter().enumerate() {
                if i == skill_tokens.len() - 1 {
                    file_path = file_path.join(format!("{}.md", token));
                } else {
                    file_path = file_path.join(token);
                }
            }
            if file_path.exists() {
                let extra_args = args[1 + skill_len..].to_vec();
                let source = make_package_source(first.to_string(), scope, ctx);
                return Ok((full_name, extra_args, source));
            }
        }
        if args.len() > 1 {
            return Err(CreftError::CommandNotFound(args[..2].join(" ")));
        }
    }

    // Check activated plugins. Each activated plugin may expose commands matching the args.
    if let Ok(resolved) = resolve_from_activated_plugins(ctx, args, scope) {
        return Ok(resolved);
    }

    Err(CreftError::CommandNotFound(args[0].clone()))
}

/// Try to resolve `args` as a command from an activated plugin in the given scope.
///
/// Loads activation settings for `scope`. For `All`-activated plugins, tries longest
/// match against skill files in the plugin directory. For `Commands`-activated plugins,
/// only checks the listed command names. Returns `CommandNotFound` if no match.
fn resolve_from_activated_plugins(
    ctx: &AppContext,
    args: &[String],
    scope: Scope,
) -> Result<(String, Vec<String>, SkillSource), CreftError> {
    let settings = registry::load_settings(ctx, scope)?;
    if settings.activated.is_empty() {
        return Err(CreftError::CommandNotFound(args[0].clone()));
    }

    let plugins_dir = ctx.plugins_dir()?;

    for (plugin_name, entry) in &settings.activated {
        let plugin_dir = plugins_dir.join(plugin_name);
        if !plugin_dir.is_dir() {
            // Stale activation — plugin was uninstalled. Skip silently here;
            // `creft doctor` reports stale activations explicitly.
            continue;
        }

        match entry {
            ActivationEntry::All(true) => {
                // Try longest match across all skill files in this plugin.
                for len in (1..=args.len().min(4)).rev() {
                    let candidate = args[..len].join(" ");
                    if let Ok(path) = registry::plugin_skill_file_path(ctx, plugin_name, &candidate)
                        && path.exists()
                    {
                        return Ok((
                            candidate,
                            args[len..].to_vec(),
                            SkillSource::Plugin(plugin_name.clone()),
                        ));
                    }
                }
            }
            ActivationEntry::Commands(cmds) => {
                // Only check the explicitly activated command names.
                for cmd in cmds {
                    let cmd_parts: Vec<&str> = cmd.split_whitespace().collect();
                    let len = cmd_parts.len();
                    if args.len() >= len
                        && args[..len].join(" ") == *cmd
                        && let Ok(path) = registry::plugin_skill_file_path(ctx, plugin_name, cmd)
                        && path.exists()
                    {
                        return Ok((
                            cmd.clone(),
                            args[len..].to_vec(),
                            SkillSource::Plugin(plugin_name.clone()),
                        ));
                    }
                }
            }
            ActivationEntry::All(false) => {
                // Normalized away by validate(); never reachable in practice.
            }
        }
    }

    Err(CreftError::CommandNotFound(args[0].clone()))
}

/// Returns true if the source is local-scope (owned or package).
///
/// `Plugin(_)` is always global — plugins live in the global cache.
pub(crate) fn is_local_source(source: &SkillSource) -> bool {
    matches!(
        source,
        SkillSource::Owned {
            scope: Scope::Local,
            ..
        } | SkillSource::Package {
            scope: Scope::Local,
            ..
        }
    )
}

/// Resolve a command name from raw CLI args.
///
/// Resolution order:
/// 1. If `creft_home` is set: use only that single location.
/// 2. Try every local root nearest-first (owned commands, then packages, then
///    activated plugins for that root). The first hit wins and records the
///    owning root on the returned `SkillSource`.
/// 3. Try global scope.
///
/// Returns `(command_name, remaining_args, SkillSource)`.
pub fn resolve_command(
    ctx: &AppContext,
    args: &[String],
) -> Result<(String, Vec<String>, SkillSource), CreftError> {
    if args.is_empty() {
        return Err(CreftError::CommandNotFound(String::new()));
    }

    if ctx.creft_home.is_some() {
        return resolve_in_scope(ctx, args, Scope::Global);
    }

    // Walk every local root nearest-first. Pin the context to each root so
    // resolve_in_scope operates on that single root and emits sources whose
    // root field reflects the pinned root.
    for local_root in ctx.local_roots() {
        let pinned = pin_ctx_to_root(ctx, local_root);
        if let Ok(result) = resolve_in_scope(&pinned, args, Scope::Local) {
            return Ok(result);
        }
    }

    resolve_in_scope(ctx, args, Scope::Global)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[allow(unused_imports)]
    use pretty_assertions::{assert_eq, assert_ne};
    use rstest::rstest;

    #[test]
    fn test_is_reserved() {
        // Top-level builtins are reserved.
        assert!(is_reserved("add"));
        assert!(is_reserved("alias"));
        assert!(is_reserved("completions"));
        assert!(is_reserved("doctor"));
        assert!(is_reserved("init"));
        assert!(is_reserved("list"));
        assert!(is_reserved("plugin"));
        assert!(is_reserved("remove"));
        assert!(is_reserved("settings"));
        assert!(is_reserved("show"));
        assert!(is_reserved("skills"));
        assert!(is_reserved("up"));
        assert!(is_reserved("update"));
        // Former namespace names are no longer reserved.
        assert!(!is_reserved("cmd"));
        assert!(!is_reserved("plugins"));
        // User-defined names are never reserved.
        assert!(!is_reserved("hello"));
        assert!(!is_reserved("gh"));
    }

    #[test]
    fn test_validate_name_ok() {
        assert!(validate_name("hello").is_ok());
        assert!(validate_name("gh issue-body").is_ok());
        assert!(validate_name("my_cmd").is_ok());
        // Former namespace names are now valid skill names.
        assert!(validate_name("cmd").is_ok());
        assert!(validate_name("plugins").is_ok());
    }

    #[test]
    fn test_validate_name_reserved() {
        assert!(matches!(
            validate_name("add"),
            Err(CreftError::ReservedName(_))
        ));
        assert!(matches!(
            validate_name("plugin"),
            Err(CreftError::ReservedName(_))
        ));
        assert!(matches!(
            validate_name("skills"),
            Err(CreftError::ReservedName(ref n)) if n == "skills"
        ));
    }

    #[test]
    fn test_validate_name_empty() {
        assert!(validate_name("").is_err());
    }

    #[test]
    fn test_validate_name_invalid_chars() {
        assert!(validate_name("hello world!").is_err());
        assert!(validate_name("cmd;rm").is_err());
    }

    // --- is_local_source unit tests ---

    #[test]
    fn test_is_local_source_owned_local() {
        let tmp = tempfile::TempDir::new().unwrap();
        assert!(is_local_source(&SkillSource::owned_local(
            tmp.path().to_path_buf()
        )));
    }

    #[test]
    fn test_is_local_source_owned_global() {
        assert!(!is_local_source(&SkillSource::owned_global()));
    }

    #[test]
    fn test_is_local_source_package_local() {
        let tmp = tempfile::TempDir::new().unwrap();
        assert!(is_local_source(&SkillSource::package_local(
            "mypkg".to_string(),
            tmp.path().to_path_buf()
        )));
    }

    #[test]
    fn test_is_local_source_package_global() {
        assert!(!is_local_source(&SkillSource::package_global(
            "mypkg".to_string()
        )));
    }

    // --- validate_path_token unit tests ---

    #[rstest]
    #[case::dot(".")]
    #[case::dotdot("..")]
    #[case::slash("a/b")]
    #[case::backslash("a\\b")]
    #[case::empty("")]
    fn validate_path_token_rejects_invalid(#[case] input: &str) {
        assert!(matches!(
            validate_path_token(input),
            Err(CreftError::InvalidName(_))
        ));
    }

    #[test]
    fn test_validate_path_token_accepts_valid() {
        assert!(validate_path_token("hello").is_ok());
        assert!(validate_path_token("my-skill").is_ok());
        assert!(validate_path_token("foo_bar").is_ok());
    }

    #[test]
    fn test_name_to_path_simple() {
        let ctx = AppContext::for_test_with_creft_home(
            PathBuf::from("/tmp/creft-test"),
            PathBuf::from("/tmp"),
        );
        let path = name_to_path_in(&ctx, "hello", Scope::Global).unwrap();
        assert_eq!(path, PathBuf::from("/tmp/creft-test/commands/hello.md"));
    }

    #[test]
    fn test_name_to_path_namespaced() {
        let ctx = AppContext::for_test_with_creft_home(
            PathBuf::from("/tmp/creft-test"),
            PathBuf::from("/tmp"),
        );
        let path = name_to_path_in(&ctx, "gh issue-body", Scope::Global).unwrap();
        assert_eq!(
            path,
            PathBuf::from("/tmp/creft-test/commands/gh/issue-body.md")
        );
    }

    #[test]
    fn test_global_root_contains_dot_creft() {
        // We can't assert a specific path (depends on host), but we can assert
        // the last component is ".creft".
        let home_dir = tempfile::TempDir::new().unwrap();
        let ctx =
            AppContext::for_test(home_dir.path().to_path_buf(), home_dir.path().to_path_buf());
        let root = ctx.global_root().unwrap();
        assert_eq!(root.file_name().and_then(|n| n.to_str()), Some(".creft"));
    }

    #[test]
    fn test_resolve_root_creft_home_overrides_both_scopes() {
        let ctx = AppContext::for_test_with_creft_home(
            PathBuf::from("/tmp/creft-override"),
            PathBuf::from("/tmp"),
        );
        assert_eq!(
            ctx.resolve_root(Scope::Global).unwrap(),
            PathBuf::from("/tmp/creft-override")
        );
        assert_eq!(
            ctx.resolve_root(Scope::Local).unwrap(),
            PathBuf::from("/tmp/creft-override")
        );
    }

    #[test]
    fn test_resolve_root_global_scope_returns_global_root() {
        let home_dir = tempfile::TempDir::new().unwrap();
        let ctx =
            AppContext::for_test(home_dir.path().to_path_buf(), home_dir.path().to_path_buf());
        let root = ctx.resolve_root(Scope::Global).unwrap();
        assert_eq!(root.file_name().and_then(|n| n.to_str()), Some(".creft"));
    }

    #[test]
    fn test_commands_dir_for_uses_scope() {
        let ctx = AppContext::for_test_with_creft_home(
            PathBuf::from("/tmp/creft-test-scope"),
            PathBuf::from("/tmp"),
        );
        let dir = ctx.commands_dir_for(Scope::Global).unwrap();
        assert_eq!(dir, PathBuf::from("/tmp/creft-test-scope/commands"));
    }

    #[test]
    fn test_default_write_scope_creft_home_mode_is_global() {
        let ctx = AppContext::for_test_with_creft_home(
            PathBuf::from("/tmp/creft-test-default"),
            PathBuf::from("/tmp"),
        );
        assert_eq!(ctx.default_write_scope(), Scope::Global);
    }

    #[test]
    fn test_default_write_scope_no_local_root_is_global() {
        // CWD has no .creft/ so the result is Global.
        let dir = tempfile::TempDir::new().unwrap();
        let home_dir = tempfile::TempDir::new().unwrap();
        let ctx = AppContext::for_test(home_dir.path().to_path_buf(), dir.path().to_path_buf());
        assert_eq!(ctx.default_write_scope(), Scope::Global);
    }

    #[test]
    fn test_default_write_scope_local_root_is_local() {
        // Create a .creft/ in a temp dir and set CWD there — default scope must be Local.
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join(".creft")).unwrap();
        let home_dir = tempfile::TempDir::new().unwrap();
        let ctx = AppContext::for_test(home_dir.path().to_path_buf(), dir.path().to_path_buf());
        assert_eq!(ctx.default_write_scope(), Scope::Local);
    }

    #[test]
    fn test_resolve_root_local_scope_returns_local_root() {
        // Set CWD to a dir that has a .creft/ — resolve_root(Local) must return it.
        let dir = tempfile::TempDir::new().unwrap();
        let creft_dir = dir.path().join(".creft");
        std::fs::create_dir_all(&creft_dir).unwrap();
        // Canonicalize to resolve symlinks (e.g. /tmp → /private/var/... on macOS).
        let creft_dir_canonical = creft_dir.canonicalize().unwrap();
        let home_dir = tempfile::TempDir::new().unwrap();
        let ctx = AppContext::for_test(
            home_dir.path().to_path_buf(),
            dir.path().canonicalize().unwrap(),
        );
        let root = ctx.resolve_root(Scope::Local).unwrap();
        assert_eq!(root, creft_dir_canonical);
    }

    #[test]
    fn test_resolve_root_local_scope_falls_back_to_global() {
        // No .creft/ in CWD — resolve_root(Local) must fall back to global_root().
        let dir = tempfile::TempDir::new().unwrap();
        let home_dir = tempfile::TempDir::new().unwrap();
        let ctx = AppContext::for_test(home_dir.path().to_path_buf(), dir.path().to_path_buf());
        let root = ctx.resolve_root(Scope::Local).unwrap();
        assert_eq!(root, ctx.global_root().unwrap());
    }

    #[test]
    fn test_name_to_path_in_uses_scope() {
        let ctx = AppContext::for_test_with_creft_home(
            PathBuf::from("/tmp/creft-test-path"),
            PathBuf::from("/tmp"),
        );
        let path = name_to_path_in(&ctx, "hello", Scope::Global).unwrap();
        assert_eq!(
            path,
            PathBuf::from("/tmp/creft-test-path/commands/hello.md")
        );
    }

    // --- walk_local_roots_from tests ---

    #[test]
    fn test_walk_local_roots_from_finds_creft_at_start() {
        // .creft/ exists directly in the start directory — returned as single entry.
        let dir = tempfile::TempDir::new().unwrap();
        let creft_dir = dir.path().join(".creft");
        std::fs::create_dir_all(&creft_dir).unwrap();

        let result = walk_local_roots_from(dir.path());
        assert_eq!(result, vec![creft_dir]);
    }

    #[test]
    fn test_walk_local_roots_from_finds_creft_in_parent() {
        // .creft/ exists in the parent of start — walk-up must find it.
        let parent = tempfile::TempDir::new().unwrap();
        let creft_dir = parent.path().join(".creft");
        std::fs::create_dir_all(&creft_dir).unwrap();

        // Child directory inside the parent (no .creft/ of its own).
        let child = parent.path().join("subdir");
        std::fs::create_dir_all(&child).unwrap();

        let result = walk_local_roots_from(&child);
        assert_eq!(result, vec![creft_dir]);
    }

    #[test]
    fn test_walk_local_roots_from_returns_empty_when_absent() {
        // No .creft/ anywhere in the temp dir tree — must return empty Vec.
        // TempDir is created under /tmp which is outside any real .creft/ tree.
        let dir = tempfile::TempDir::new().unwrap();
        let child = dir.path().join("a").join("b").join("c");
        std::fs::create_dir_all(&child).unwrap();

        let result = walk_local_roots_from(&child);
        assert!(
            result.is_empty(),
            "expected empty Vec when no .creft/ exists, got {:?}",
            result
        );
    }

    #[test]
    fn test_walk_local_roots_from_skips_creft_file() {
        // .creft exists as a file (not a directory) — must be skipped, returning empty.
        let dir = tempfile::TempDir::new().unwrap();
        let creft_file = dir.path().join(".creft");
        std::fs::write(&creft_file, "not a directory").unwrap();

        let result = walk_local_roots_from(dir.path());
        assert!(
            result.is_empty(),
            "expected empty Vec when .creft is a file, got {:?}",
            result
        );
    }

    #[test]
    fn test_walk_local_roots_from_two_entries_nearest_first() {
        // Intermediate .creft/ and ancestor .creft/ — both returned, nearest first.
        let root = tempfile::TempDir::new().unwrap();
        let ancestor_creft = root.path().join(".creft");
        std::fs::create_dir_all(&ancestor_creft).unwrap();
        let infra = root.path().join("infra").join("rackroom");
        let infra_creft = infra.join(".creft");
        std::fs::create_dir_all(&infra_creft).unwrap();
        let cwd = infra.join("deploy");
        std::fs::create_dir_all(&cwd).unwrap();

        let result = walk_local_roots_from(&cwd);
        assert_eq!(result.len(), 2, "expected two entries, got {:?}", result);
        assert_eq!(result[0], infra_creft, "nearest root must come first");
        assert_eq!(result[1], ancestor_creft, "ancestor root must come second");
    }

    #[test]
    fn test_walk_local_roots_from_skips_creft_file_at_intermediate_depth() {
        // .creft at the start is a file (not a directory) — skip it.
        // .creft at the parent is a real directory — must be included.
        let parent = tempfile::TempDir::new().unwrap();
        let parent_creft = parent.path().join(".creft");
        std::fs::create_dir_all(&parent_creft).unwrap();
        let child = parent.path().join("child");
        std::fs::create_dir_all(&child).unwrap();
        let child_creft = child.join(".creft");
        std::fs::write(&child_creft, "file not dir").unwrap();

        let result = walk_local_roots_from(&child);
        assert_eq!(result, vec![parent_creft]);
    }

    #[test]
    fn test_resolve_command_rejects_traversal() {
        let dir = tempfile::TempDir::new().unwrap();
        let ctx = AppContext::for_test_with_creft_home(
            dir.path().to_path_buf(),
            dir.path().to_path_buf(),
        );

        // Create a package directory so the package-resolution branch is entered.
        let pkg_dir = dir.path().join("packages").join("mypkg");
        std::fs::create_dir_all(&pkg_dir).unwrap();

        // Create a skill file that would be reachable with a normal name.
        std::fs::write(
            pkg_dir.join("something.md"),
            "---\nname: mypkg something\ndescription: test\n---\n\n```bash\necho ok\n```\n",
        )
        .unwrap();

        // Attempt path traversal: args contain ".." tokens.
        let args: Vec<String> = vec![
            "mypkg".to_string(),
            "..".to_string(),
            "something".to_string(),
        ];
        let result = resolve_command(&ctx, &args);
        assert!(
            result.is_err(),
            "expected error for traversal args, got {:?}",
            result
        );
    }

    // --- symlink skipping tests ---

    #[cfg(unix)]
    #[test]
    fn test_collect_commands_skips_symlinks() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::TempDir::new().unwrap();
        let ctx = AppContext::for_test_with_creft_home(
            dir.path().to_path_buf(),
            dir.path().to_path_buf(),
        );

        let cmd_dir = dir.path().join("commands");
        std::fs::create_dir_all(&cmd_dir).unwrap();

        // Regular command file.
        std::fs::write(
            cmd_dir.join("real.md"),
            "---\nname: real\ndescription: real command\n---\n\n```bash\necho real\n```\n",
        )
        .unwrap();

        // A command file outside the commands directory that we symlink in.
        let outside = dir.path().join("outside.md");
        std::fs::write(
            &outside,
            "---\nname: outside\ndescription: outside command\n---\n\n```bash\necho outside\n```\n",
        )
        .unwrap();
        symlink(&outside, cmd_dir.join("linked.md")).unwrap();

        let all = list_all_in(&ctx, Scope::Global).unwrap();
        let names: Vec<&str> = all.iter().map(|d| d.name.as_str()).collect();

        assert_eq!(
            all.len(),
            1,
            "only the real command should be listed, got: {:?}",
            names
        );
        assert_eq!(names[0], "real");
    }

    #[test]
    fn collect_commands_silently_skips_plain_markdown_without_frontmatter() {
        let dir = tempfile::TempDir::new().unwrap();
        let ctx = AppContext::for_test_with_creft_home(
            dir.path().to_path_buf(),
            dir.path().to_path_buf(),
        );

        let cmd_dir = dir.path().join("commands");
        std::fs::create_dir_all(&cmd_dir).unwrap();

        std::fs::write(
            cmd_dir.join("real.md"),
            "---\nname: real\ndescription: real command\n---\n\n```bash\necho real\n```\n",
        )
        .unwrap();

        // Plain markdown files without frontmatter (README, CHANGELOG, NOTES, etc.)
        // must be silently skipped — no warning, no inclusion as a command.
        std::fs::write(
            cmd_dir.join("README.md"),
            "# Commands\n\nDocumentation here.\n",
        )
        .unwrap();
        std::fs::write(cmd_dir.join("NOTES.md"), "# Notes\n\nSome notes.\n").unwrap();

        let all = list_all_in(&ctx, Scope::Global).unwrap();
        let names: Vec<&str> = all.iter().map(|d| d.name.as_str()).collect();

        assert_eq!(
            all.len(),
            1,
            "plain markdown files without frontmatter must not appear as commands, got: {:?}",
            names
        );
        assert_eq!(names[0], "real");
    }

    #[test]
    fn collect_commands_excludes_invalid_yaml_frontmatter_file_from_results() {
        let dir = tempfile::TempDir::new().unwrap();
        let ctx = AppContext::for_test_with_creft_home(
            dir.path().to_path_buf(),
            dir.path().to_path_buf(),
        );

        let cmd_dir = dir.path().join("commands");
        std::fs::create_dir_all(&cmd_dir).unwrap();

        std::fs::write(
            cmd_dir.join("real.md"),
            "---\nname: real\ndescription: real command\n---\n\n```bash\necho real\n```\n",
        )
        .unwrap();

        // A file with opening and closing delimiters but invalid YAML inside:
        // this is a `Frontmatter` error (not `MissingFrontmatterDelimiter`), so
        // it emits a warning rather than being silently skipped.
        std::fs::write(
            cmd_dir.join("broken.md"),
            "---\n: invalid: yaml: [\n---\n\n```bash\necho broken\n```\n",
        )
        .unwrap();

        let all = list_all_in(&ctx, Scope::Global).unwrap();
        let names: Vec<&str> = all.iter().map(|d| d.name.as_str()).collect();

        assert_eq!(
            all.len(),
            1,
            "invalid-yaml-frontmatter file must not appear as a command, got: {:?}",
            names
        );
        assert_eq!(names[0], "real");
    }

    // --- scope-aware resolution tests ---
    //
    // These tests exercise local/global split WITHOUT CREFT_HOME so that the two-tier
    // resolution path is exercised. They use AppContext::for_test() with explicit temp
    // directories — no env vars or CWD mutation needed.

    fn make_skill(name: &str, desc: &str) -> String {
        format!("---\nname: {name}\ndescription: {desc}\n---\n\n```bash\necho {name}\n```\n")
    }

    /// Writes a skill file to `<root>/commands/<name>.md`, creating directories as needed.
    ///
    /// For simple names (no spaces), the file is at `commands/<name>.md`.
    /// For namespaced names (with spaces), the file is at the flat path
    /// `commands/<name with spaces>.md`, which `list_all_in` discovers via
    /// filesystem walk. Use `write_skill_dir` when directory-structured paths
    /// are required (e.g., for `read_raw_from` or `load_from` tests).
    fn write_skill_to_root(root: &std::path::Path, name: &str, desc: &str) {
        let path = root.join("commands").join(format!("{}.md", name));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, make_skill(name, desc)).unwrap();
    }

    /// Writes a skill file using directory structure for namespaced names.
    ///
    /// `"deploy rollback"` → `<root>/commands/deploy/rollback.md`
    ///
    /// This matches the path that `read_raw_from`, `load_from`, and `path_in_root`
    /// expect for local-scoped skills. Use this when the test needs to read or
    /// load the skill after resolving it.
    fn write_skill_dir(root: &std::path::Path, name: &str, desc: &str) {
        let parts: Vec<&str> = name.split_whitespace().collect();
        let mut path = root.join("commands");
        for part in &parts[..parts.len().saturating_sub(1)] {
            path = path.join(part);
        }
        if let Some(leaf) = parts.last() {
            path = path.join(format!("{}.md", leaf));
        }
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, make_skill(name, desc)).unwrap();
    }

    // --- scope-aware save, load, remove tests ---

    #[test]
    fn test_save_to_local_scope_writes_to_local_commands_dir() {
        let home_dir = tempfile::TempDir::new().unwrap();
        let project_dir = tempfile::TempDir::new().unwrap();

        // Create a local .creft/ directory
        let local_root = project_dir.path().join(".creft");
        std::fs::create_dir_all(&local_root).unwrap();

        let ctx = AppContext::for_test(
            home_dir.path().to_path_buf(),
            project_dir.path().to_path_buf(),
        );

        let content = make_skill("my-local", "local skill");
        let name = save(&ctx, &content, false, Scope::Local).unwrap();

        assert_eq!(name, "my-local");

        // Verify the file was written to the local commands directory, not global
        let local_path = local_root.join("commands").join("my-local.md");
        let global_path = home_dir
            .path()
            .join(".creft")
            .join("commands")
            .join("my-local.md");
        assert!(
            local_path.exists(),
            "skill must be written to local .creft/commands/"
        );
        assert!(
            !global_path.exists(),
            "skill must NOT be written to global ~/.creft/commands/"
        );
    }

    #[test]
    fn test_save_to_global_scope_writes_to_global_commands_dir() {
        let home_dir = tempfile::TempDir::new().unwrap();
        let project_dir = tempfile::TempDir::new().unwrap();

        // Create a local .creft/ directory (save --global should bypass it)
        let local_root = project_dir.path().join(".creft");
        std::fs::create_dir_all(&local_root).unwrap();

        let ctx = AppContext::for_test(
            home_dir.path().to_path_buf(),
            project_dir.path().to_path_buf(),
        );

        let content = make_skill("my-global", "global skill");
        let name = save(&ctx, &content, false, Scope::Global).unwrap();

        assert_eq!(name, "my-global");

        // Verify the file was written to the global commands directory, not local
        let local_path = local_root.join("commands").join("my-global.md");
        let global_path = home_dir
            .path()
            .join(".creft")
            .join("commands")
            .join("my-global.md");
        assert!(
            global_path.exists(),
            "skill must be written to global ~/.creft/commands/"
        );
        assert!(
            !local_path.exists(),
            "skill must NOT be written to local .creft/commands/"
        );
    }

    #[test]
    fn test_load_in_loads_from_correct_scope() {
        let home_dir = tempfile::TempDir::new().unwrap();
        let project_dir = tempfile::TempDir::new().unwrap();

        let local_root = project_dir.path().join(".creft");
        let global_root = home_dir.path().join(".creft");

        // Write "hello" to both scopes with different descriptions to distinguish them
        write_skill_to_root(&local_root, "hello", "local hello");
        write_skill_to_root(&global_root, "hello", "global hello");

        let ctx = AppContext::for_test(
            home_dir.path().to_path_buf(),
            project_dir.path().to_path_buf(),
        );

        let local_cmd = load_in(&ctx, "hello", Scope::Local).unwrap();
        let global_cmd = load_in(&ctx, "hello", Scope::Global).unwrap();

        assert_eq!(local_cmd.def.description, "local hello");
        assert_eq!(global_cmd.def.description, "global hello");
    }

    #[test]
    fn test_remove_in_deletes_from_correct_scope() {
        let home_dir = tempfile::TempDir::new().unwrap();
        let project_dir = tempfile::TempDir::new().unwrap();

        let local_root = project_dir.path().join(".creft");
        let global_root_path = home_dir.path().join(".creft");

        // Write "cleanup" to both scopes
        write_skill_to_root(&local_root, "cleanup", "local cleanup");
        write_skill_to_root(&global_root_path, "cleanup", "global cleanup");

        let ctx = AppContext::for_test(
            home_dir.path().to_path_buf(),
            project_dir.path().to_path_buf(),
        );

        // Remove from local scope only
        remove_in(&ctx, "cleanup", Scope::Local).unwrap();

        // Local file should be gone; global should remain
        let local_path = local_root.join("commands").join("cleanup.md");
        let global_path = global_root_path.join("commands").join("cleanup.md");
        assert!(!local_path.exists(), "local skill must be removed");
        assert!(global_path.exists(), "global skill must remain untouched");
    }

    #[test]
    fn test_list_all_with_source_merges_scopes_local_shadows_global() {
        // local has "hello" and "local-only"; global has "hello" (shadowed) and "global-only".
        // Expected result: hello (Local), local-only (Local), global-only (Global).
        let home_dir = tempfile::TempDir::new().unwrap();
        let project_dir = tempfile::TempDir::new().unwrap();

        let global_root_path = home_dir.path().join(".creft");
        write_skill_to_root(&global_root_path, "hello", "global hello");
        write_skill_to_root(&global_root_path, "global-only", "global only skill");

        let local_root = project_dir.path().join(".creft");
        write_skill_to_root(&local_root, "hello", "local hello");
        write_skill_to_root(&local_root, "local-only", "local only skill");

        let ctx = AppContext::for_test(
            home_dir.path().to_path_buf(),
            project_dir.path().to_path_buf(),
        );

        let items = list_all_with_source(&ctx).expect("list_all_with_source should succeed");

        // All three unique names must appear exactly once.
        let by_name: std::collections::HashMap<&str, &SkillSource> =
            items.iter().map(|(d, s)| (d.name.as_str(), s)).collect();

        assert_eq!(
            by_name.len(),
            3,
            "expected 3 unique skills, got: {:?}",
            items.iter().map(|(d, _)| &d.name).collect::<Vec<_>>()
        );

        assert_eq!(
            by_name.get("hello").map(|s| s.scope()),
            Some(Scope::Local),
            "local hello must shadow global hello"
        );
        assert_eq!(
            by_name.get("local-only").map(|s| s.scope()),
            Some(Scope::Local),
            "local-only must appear with Local scope"
        );
        assert_eq!(
            by_name.get("global-only").map(|s| s.scope()),
            Some(Scope::Global),
            "global-only must appear with Global scope"
        );
        // Local sources must carry the owning root.
        assert_eq!(
            by_name.get("hello").and_then(|s| s.local_root()),
            Some(local_root.as_path()),
            "local hello must carry the owning local root"
        );
        assert_eq!(
            by_name.get("global-only").and_then(|s| s.local_root()),
            None,
            "global-only must have no owning root"
        );
    }

    // --- has_local_root unit tests ---

    #[test]
    fn test_has_local_root_exists() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join(".creft")).unwrap();
        assert!(has_local_root(tmp.path()).is_some());
    }

    #[test]
    fn test_has_local_root_missing() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(has_local_root(tmp.path()).is_none());
    }

    #[test]
    fn test_has_local_root_file_not_dir() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join(".creft"), "not a dir").unwrap();
        assert!(has_local_root(tmp.path()).is_none());
    }

    // --- walk_parent_local_roots unit tests ---

    #[test]
    fn test_walk_parent_local_roots_found() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join(".creft")).unwrap();
        let child = tmp.path().join("child");
        std::fs::create_dir(&child).unwrap();
        let result = walk_parent_local_roots(&child);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], tmp.path().join(".creft"));
    }

    #[test]
    fn test_walk_parent_local_roots_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        let child = tmp.path().join("child");
        std::fs::create_dir(&child).unwrap();
        // No .creft/ anywhere in the tempdir hierarchy
        assert!(walk_parent_local_roots(&child).is_empty());
    }

    #[test]
    fn test_walk_parent_local_roots_skips_start() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join(".creft")).unwrap();
        // Start is the dir that HAS .creft/ -- walk_parent skips start itself
        assert!(walk_parent_local_roots(tmp.path()).is_empty());
    }

    #[test]
    fn test_is_reserved_init() {
        assert!(is_reserved("init"));
    }

    // --- group_by_namespace unit tests ---

    fn make_owned_skill(name: &str, desc: &str) -> (CommandDef, SkillSource) {
        let def = CommandDef {
            name: name.to_string(),
            description: desc.to_string(),
            args: vec![],
            flags: vec![],
            env: vec![],
            tags: vec![],
            supports: vec![],
        };
        (def, SkillSource::owned_global())
    }

    fn make_pkg_skill(name: &str, desc: &str, pkg: &str) -> (CommandDef, SkillSource) {
        let def = CommandDef {
            name: name.to_string(),
            description: desc.to_string(),
            args: vec![],
            flags: vec![],
            env: vec![],
            tags: vec![],
            supports: vec![],
        };
        (def, SkillSource::package_global(pkg.to_string()))
    }

    #[test]
    fn test_group_by_namespace_top_level() {
        // Given skills ["hello", "tavily search", "tavily crawl", "gh issue-body"],
        // top-level grouping produces:
        //   Namespace("gh", 1, None), Namespace("tavily", 2, None), Skill("hello")
        // Namespaces sorted first, then skills.
        let skills = vec![
            make_owned_skill("hello", "Greets someone"),
            make_owned_skill("tavily search", "Search the web"),
            make_owned_skill("tavily crawl", "Crawl a website"),
            make_owned_skill("gh issue-body", "Fetch issue body"),
        ];

        let result = group_by_namespace(skills, &[]);

        assert_eq!(result.len(), 3);

        // First: gh namespace
        match &result[0] {
            NamespaceEntry::Namespace {
                name,
                skill_count,
                package,
            } => {
                assert_eq!(name, "gh");
                assert_eq!(*skill_count, 1);
                assert!(package.is_none());
            }
            NamespaceEntry::Skill(_, _) => panic!("expected Namespace, got Skill"),
        }

        // Second: tavily namespace
        match &result[1] {
            NamespaceEntry::Namespace {
                name,
                skill_count,
                package,
            } => {
                assert_eq!(name, "tavily");
                assert_eq!(*skill_count, 2);
                assert!(package.is_none());
            }
            NamespaceEntry::Skill(_, _) => panic!("expected Namespace, got Skill"),
        }

        // Third: hello skill
        match &result[2] {
            NamespaceEntry::Skill(def, _) => {
                assert_eq!(def.name, "hello");
            }
            NamespaceEntry::Namespace { .. } => panic!("expected Skill, got Namespace"),
        }
    }

    #[test]
    fn test_group_by_namespace_drill_in() {
        // Same input, prefix ["tavily"] produces: Skill("tavily crawl"), Skill("tavily search").
        // Both are leaf skills at this level, sorted alphabetically.
        let skills = vec![
            make_owned_skill("hello", "Greets someone"),
            make_owned_skill("tavily search", "Search the web"),
            make_owned_skill("tavily crawl", "Crawl a website"),
            make_owned_skill("gh issue-body", "Fetch issue body"),
        ];

        let result = group_by_namespace(skills, &["tavily"]);

        assert_eq!(result.len(), 2);

        match &result[0] {
            NamespaceEntry::Skill(def, _) => assert_eq!(def.name, "tavily crawl"),
            NamespaceEntry::Namespace { .. } => panic!("expected Skill, got Namespace"),
        }
        match &result[1] {
            NamespaceEntry::Skill(def, _) => assert_eq!(def.name, "tavily search"),
            NamespaceEntry::Namespace { .. } => panic!("expected Skill, got Namespace"),
        }
    }

    #[test]
    fn test_group_by_namespace_deep_nesting() {
        // Skills: ["aws s3 copy", "aws s3 sync", "aws ec2 list"]
        // Prefix ["aws"]: Namespace("aws ec2", 1, None), Namespace("aws s3", 2, None)
        // Prefix ["aws", "s3"]: Skill("aws s3 copy"), Skill("aws s3 sync")
        let skills = vec![
            make_owned_skill("aws s3 copy", "Copy objects between S3 buckets"),
            make_owned_skill("aws s3 sync", "Sync a local directory to S3"),
            make_owned_skill("aws ec2 list", "List EC2 instances"),
        ];

        // Drill into "aws"
        let aws_result = group_by_namespace(skills.clone(), &["aws"]);
        assert_eq!(aws_result.len(), 2);

        match &aws_result[0] {
            NamespaceEntry::Namespace {
                name,
                skill_count,
                package,
            } => {
                assert_eq!(name, "aws ec2");
                assert_eq!(*skill_count, 1);
                assert!(package.is_none());
            }
            NamespaceEntry::Skill(_, _) => panic!("expected Namespace"),
        }
        match &aws_result[1] {
            NamespaceEntry::Namespace {
                name,
                skill_count,
                package,
            } => {
                assert_eq!(name, "aws s3");
                assert_eq!(*skill_count, 2);
                assert!(package.is_none());
            }
            NamespaceEntry::Skill(_, _) => panic!("expected Namespace"),
        }

        // Drill into "aws s3"
        let s3_result = group_by_namespace(skills, &["aws", "s3"]);
        assert_eq!(s3_result.len(), 2);

        match &s3_result[0] {
            NamespaceEntry::Skill(def, _) => assert_eq!(def.name, "aws s3 copy"),
            NamespaceEntry::Namespace { .. } => panic!("expected Skill"),
        }
        match &s3_result[1] {
            NamespaceEntry::Skill(def, _) => assert_eq!(def.name, "aws s3 sync"),
            NamespaceEntry::Namespace { .. } => panic!("expected Skill"),
        }
    }

    #[test]
    fn test_group_by_namespace_package_detection() {
        // Skills where all k8s-tools skills come from the k8s-tools package.
        let skills = vec![
            make_pkg_skill("k8s-tools apply", "Apply manifests", "k8s-tools"),
            make_pkg_skill("k8s-tools get", "Get resources", "k8s-tools"),
            make_pkg_skill("k8s-tools delete", "Delete resources", "k8s-tools"),
        ];

        let result = group_by_namespace(skills, &[]);

        assert_eq!(result.len(), 1);
        match &result[0] {
            NamespaceEntry::Namespace {
                name,
                skill_count,
                package,
            } => {
                assert_eq!(name, "k8s-tools");
                assert_eq!(*skill_count, 3);
                assert_eq!(package.as_deref(), Some("k8s-tools"));
            }
            NamespaceEntry::Skill(_, _) => panic!("expected Namespace"),
        }
    }

    #[test]
    fn test_group_by_namespace_mixed_package() {
        // One "tavily search" is owned, one "tavily crawl" is from package.
        // Produces Namespace("tavily", 2, None) -- no package annotation.
        let skills = vec![
            make_owned_skill("tavily search", "Search the web"),
            make_pkg_skill("tavily crawl", "Crawl a website", "tavily"),
        ];

        let result = group_by_namespace(skills, &[]);

        assert_eq!(result.len(), 1);
        match &result[0] {
            NamespaceEntry::Namespace {
                name,
                skill_count,
                package,
            } => {
                assert_eq!(name, "tavily");
                assert_eq!(*skill_count, 2);
                assert!(
                    package.is_none(),
                    "mixed namespace must not have package annotation"
                );
            }
            NamespaceEntry::Skill(_, _) => panic!("expected Namespace"),
        }
    }

    #[test]
    fn test_group_by_namespace_single_skill() {
        // Only "gh issue-body" -- top-level produces Namespace("gh", 1, None).
        // Does NOT auto-expand single-skill namespaces.
        let skills = vec![make_owned_skill("gh issue-body", "Fetch issue body")];

        let result = group_by_namespace(skills, &[]);

        assert_eq!(result.len(), 1);
        match &result[0] {
            NamespaceEntry::Namespace {
                name,
                skill_count,
                package,
            } => {
                assert_eq!(name, "gh");
                assert_eq!(*skill_count, 1);
                assert!(package.is_none());
            }
            NamespaceEntry::Skill(_, _) => panic!("expected Namespace, not auto-expanded Skill"),
        }
    }

    #[test]
    fn test_group_by_namespace_empty() {
        let result = group_by_namespace(vec![], &[]);
        assert!(result.is_empty());
    }

    #[test]
    fn test_group_by_namespace_no_match() {
        let skills = vec![
            make_owned_skill("hello", "Greets someone"),
            make_owned_skill("tavily search", "Search the web"),
        ];

        let result = group_by_namespace(skills, &["nonexistent"]);
        assert!(result.is_empty());
    }

    #[test]
    fn test_group_skill_name_equals_prefix() {
        // Skill named "aws" (single token) plus "aws s3 copy".
        // Prefix ["aws"]: should NOT include the "aws" skill (0 remaining parts).
        // Should show Namespace("aws s3", 1, None) only.
        let skills = vec![
            make_owned_skill("aws", "AWS CLI wrapper"),
            make_owned_skill("aws s3 copy", "Copy objects between S3 buckets"),
        ];

        let result = group_by_namespace(skills, &["aws"]);

        assert_eq!(result.len(), 1);
        match &result[0] {
            NamespaceEntry::Namespace {
                name, skill_count, ..
            } => {
                assert_eq!(name, "aws s3");
                assert_eq!(*skill_count, 1);
            }
            NamespaceEntry::Skill(def, _) => {
                panic!("expected Namespace, got Skill({})", def.name)
            }
        }
    }

    #[test]
    fn test_namespace_exists_true() {
        let dir = tempfile::TempDir::new().unwrap();
        let ctx = AppContext::for_test_with_creft_home(
            dir.path().to_path_buf(),
            dir.path().to_path_buf(),
        );

        let content = make_skill("tavily search", "Search the web");
        save(&ctx, &content, false, Scope::Global).unwrap();

        let exists = namespace_exists(&ctx, &["tavily"]).unwrap();
        assert!(
            exists,
            "namespace_exists should return true when skills exist under 'tavily'"
        );
    }

    #[test]
    fn test_namespace_exists_false() {
        let dir = tempfile::TempDir::new().unwrap();
        let ctx = AppContext::for_test_with_creft_home(
            dir.path().to_path_buf(),
            dir.path().to_path_buf(),
        );

        let content = make_skill("hello", "Greet someone");
        save(&ctx, &content, false, Scope::Global).unwrap();

        let exists = namespace_exists(&ctx, &["tavily"]).unwrap();
        assert!(
            !exists,
            "namespace_exists should return false when no skills exist under 'tavily'"
        );
    }

    // ── skill_test_fixture_path ───────────────────────────────────────────────

    #[test]
    fn skill_test_fixture_path_for_top_level_skill() {
        let ctx = AppContext::for_test_with_creft_home(
            PathBuf::from("/tmp/creft-test"),
            PathBuf::from("/tmp"),
        );
        let path = skill_test_fixture_path(&ctx, "setup", Scope::Global).unwrap();
        assert_eq!(
            path,
            PathBuf::from("/tmp/creft-test/commands/setup.test.yaml")
        );
    }

    #[test]
    fn skill_test_fixture_path_for_namespaced_skill() {
        let ctx = AppContext::for_test_with_creft_home(
            PathBuf::from("/tmp/creft-test"),
            PathBuf::from("/tmp"),
        );
        let path = skill_test_fixture_path(&ctx, "hooks guard bash", Scope::Global).unwrap();
        assert_eq!(
            path,
            PathBuf::from("/tmp/creft-test/commands/hooks/guard/bash.test.yaml")
        );
    }

    #[test]
    fn skill_test_fixture_path_rejects_invalid_name() {
        let ctx = AppContext::for_test_with_creft_home(
            PathBuf::from("/tmp/creft-test"),
            PathBuf::from("/tmp"),
        );
        // An empty name is rejected by name_to_path_in's underlying validation.
        let err = skill_test_fixture_path(&ctx, "", Scope::Global).unwrap_err();
        assert!(
            matches!(
                err,
                CreftError::InvalidName(_) | CreftError::ReservedName(_)
            ),
            "expected InvalidName or ReservedName for empty input, got: {err}"
        );
    }

    // ── Stage 2: hierarchical resolution ─────────────────────────────────────

    /// BL-4 repro: an empty intermediate `.creft/` must not block resolution
    /// to an ancestor root's skill. The returned source must carry the ancestor
    /// root and have Local scope.
    #[test]
    fn resolve_command_falls_through_empty_intermediate_creft() {
        use pretty_assertions::assert_eq;

        let home_dir = tempfile::TempDir::new().unwrap();
        // Hierarchy: project_dir / infra / rackroom
        let project_dir = tempfile::TempDir::new().unwrap();
        let infra_dir = project_dir.path().join("infra").join("rackroom");
        std::fs::create_dir_all(&infra_dir).unwrap();

        // Ancestor root: populated with a skill.
        let ancestor_root = project_dir.path().join(".creft");
        write_skill_to_root(&ancestor_root, "remote", "remote desc");

        // Intermediate root: empty (no skills).
        let intermediate_root = infra_dir.join(".creft");
        std::fs::create_dir_all(intermediate_root.join("commands")).unwrap();

        // CWD is inside the intermediate (deepest) directory.
        let ctx = AppContext::for_test(home_dir.path().to_path_buf(), infra_dir.clone());

        let args: Vec<String> = vec!["remote".to_string()];
        let (name, remaining, source) = resolve_command(&ctx, &args)
            .expect("resolve_command must succeed through empty intermediate root");

        assert_eq!(name, "remote");
        assert!(remaining.is_empty());
        assert_eq!(
            source.scope(),
            Scope::Local,
            "skill from ancestor root must be Local-scoped"
        );
        assert_eq!(
            source.local_root(),
            Some(ancestor_root.as_path()),
            "owning root must be the ancestor .creft/, not the intermediate"
        );

        // Verify the resolved body can be read.
        let raw = read_raw_from(&ctx, "remote", &source).expect("read_raw_from must succeed");
        assert!(raw.contains("remote desc"));
    }

    /// When both ancestor and intermediate roots define the same skill, the
    /// intermediate (most-local) wins and its file is read.
    #[test]
    fn resolve_command_most_local_root_wins_on_conflict() {
        use pretty_assertions::assert_eq;

        let home_dir = tempfile::TempDir::new().unwrap();
        let project_dir = tempfile::TempDir::new().unwrap();
        let sub_dir = project_dir.path().join("sub");
        std::fs::create_dir_all(&sub_dir).unwrap();

        let ancestor_root = project_dir.path().join(".creft");
        write_skill_to_root(&ancestor_root, "remote", "ancestor remote");

        let sub_root = sub_dir.join(".creft");
        write_skill_to_root(&sub_root, "remote", "sub remote");

        let ctx = AppContext::for_test(home_dir.path().to_path_buf(), sub_dir.clone());

        let args: Vec<String> = vec!["remote".to_string()];
        let (name, _, source) = resolve_command(&ctx, &args).expect("resolve_command must succeed");

        assert_eq!(name, "remote");
        assert_eq!(
            source.local_root(),
            Some(sub_root.as_path()),
            "most-local root must win"
        );
        let raw = read_raw_from(&ctx, "remote", &source).expect("read_raw_from must succeed");
        assert!(
            raw.contains("sub remote"),
            "body must come from the sub root"
        );
    }

    /// `list_all_with_source` from a deep CWD returns skills from every ancestor
    /// root, deduplicated by name (nearest-wins), each carrying its owning root.
    #[test]
    fn list_all_with_source_returns_union_with_nearest_wins() {
        use pretty_assertions::assert_eq;

        let home_dir = tempfile::TempDir::new().unwrap();
        let project_dir = tempfile::TempDir::new().unwrap();
        let sub_dir = project_dir.path().join("sub");
        std::fs::create_dir_all(&sub_dir).unwrap();

        let ancestor_root = project_dir.path().join(".creft");
        write_skill_to_root(&ancestor_root, "shared", "ancestor shared");
        write_skill_to_root(&ancestor_root, "ancestor-only", "ancestor only");

        let sub_root = sub_dir.join(".creft");
        write_skill_to_root(&sub_root, "shared", "sub shared");
        write_skill_to_root(&sub_root, "sub-only", "sub only");

        let ctx = AppContext::for_test(home_dir.path().to_path_buf(), sub_dir.clone());

        let items = list_all_with_source(&ctx).expect("list_all_with_source must succeed");
        let by_name: std::collections::HashMap<&str, &SkillSource> =
            items.iter().map(|(d, s)| (d.name.as_str(), s)).collect();

        assert_eq!(
            by_name.len(),
            3,
            "expected 3 unique skills: shared, ancestor-only, sub-only"
        );

        // "shared" resolves to the sub (nearest) root.
        assert_eq!(
            by_name["shared"].local_root(),
            Some(sub_root.as_path()),
            "shared skill must resolve to nearest (sub) root"
        );
        // "ancestor-only" resolves to the ancestor root.
        assert_eq!(
            by_name["ancestor-only"].local_root(),
            Some(ancestor_root.as_path()),
            "ancestor-only skill must resolve to ancestor root"
        );
        // "sub-only" resolves to the sub root.
        assert_eq!(
            by_name["sub-only"].local_root(),
            Some(sub_root.as_path()),
            "sub-only skill must resolve to sub root"
        );
    }

    /// `derive_cwd` sets the subprocess CWD to the parent of the owning root,
    /// not the parent of the nearest root.
    #[test]
    fn derive_cwd_uses_owning_root_not_nearest() {
        use pretty_assertions::assert_eq;

        let home_dir = tempfile::TempDir::new().unwrap();
        let project_dir = tempfile::TempDir::new().unwrap();
        let sub_dir = project_dir.path().join("sub");
        std::fs::create_dir_all(&sub_dir).unwrap();

        let ancestor_root = project_dir.path().join(".creft");
        write_skill_to_root(&ancestor_root, "remote", "remote in ancestor");
        // Empty sub root — resolution falls through to ancestor.
        let sub_root = sub_dir.join(".creft");
        std::fs::create_dir_all(sub_root.join("commands")).unwrap();

        let ctx = AppContext::for_test(home_dir.path().to_path_buf(), sub_dir.clone());

        let args: Vec<String> = vec!["remote".to_string()];
        let (_, _, source) = resolve_command(&ctx, &args).expect("must resolve");

        // The owning root is the ancestor — derive_cwd must point there.
        let expected_cwd = project_dir.path().to_path_buf();
        let actual_cwd = ctx.derive_cwd(&source);
        assert_eq!(
            actual_cwd, expected_cwd,
            "derive_cwd must use the ancestor root's parent, not the sub root's parent"
        );
    }

    /// `local_root()` returns `Some` for local-scoped sources and `None` for global.
    #[test]
    fn skill_source_local_root_accessor_contract() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().join(".creft");

        let local_owned = SkillSource::owned_local(root.clone());
        let global_owned = SkillSource::owned_global();
        let local_pkg = SkillSource::package_local("pkg".to_string(), root.clone());
        let global_pkg = SkillSource::package_global("pkg".to_string());
        let plugin = SkillSource::Plugin("plug".to_string());

        assert_eq!(local_owned.local_root(), Some(root.as_path()));
        assert_eq!(global_owned.local_root(), None);
        assert_eq!(local_pkg.local_root(), Some(root.as_path()));
        assert_eq!(global_pkg.local_root(), None);
        assert_eq!(plugin.local_root(), None);
    }

    /// `rebuild_all_indexes` on a two-root project writes one index file per
    /// root (not collapsed into a single nearest-root file).
    #[test]
    fn rebuild_all_indexes_writes_per_root_index_files() {
        use crate::search::store as search_store;

        let home_dir = tempfile::TempDir::new().unwrap();
        let project_dir = tempfile::TempDir::new().unwrap();
        let sub_dir = project_dir.path().join("sub");
        std::fs::create_dir_all(&sub_dir).unwrap();

        let ancestor_root = project_dir.path().join(".creft");
        write_skill_dir(&ancestor_root, "deploy rollback", "rollback a deployment");

        let sub_root = sub_dir.join(".creft");
        write_skill_dir(&sub_root, "deploy push", "push a build");

        let ctx = AppContext::for_test(home_dir.path().to_path_buf(), sub_dir.clone());

        search_store::rebuild_all_indexes(&ctx).expect("rebuild_all_indexes must succeed");

        // Both roots must have a "deploy" index file.
        let ancestor_idx = ancestor_root.join("indexes").join("deploy.idx");
        let sub_idx = sub_root.join("indexes").join("deploy.idx");

        assert!(
            ancestor_idx.exists(),
            "ancestor root must have its own deploy.idx at {ancestor_idx:?}"
        );
        assert!(
            sub_idx.exists(),
            "sub root must have its own deploy.idx at {sub_idx:?}"
        );

        // Each index file must contain only the skill that lives in that root.
        let ancestor_index =
            crate::search::index::SearchIndex::from_bytes(&std::fs::read(&ancestor_idx).unwrap())
                .expect("ancestor index must be valid");
        let sub_index =
            crate::search::index::SearchIndex::from_bytes(&std::fs::read(&sub_idx).unwrap())
                .expect("sub index must be valid");

        assert_eq!(
            ancestor_index.len(),
            1,
            "ancestor index must have exactly 1 skill"
        );
        assert_eq!(sub_index.len(), 1, "sub index must have exactly 1 skill");

        let ancestor_results = ancestor_index.search("rollback");
        assert_eq!(ancestor_results.len(), 1);
        assert_eq!(ancestor_results[0].name, "deploy rollback");

        let sub_results = sub_index.search("push");
        assert_eq!(sub_results.len(), 1);
        assert_eq!(sub_results[0].name, "deploy push");
    }

    /// A package installed in an ancestor `.creft/packages/` resolves from a
    /// sub-project CWD with an empty intermediate `.creft/`. The returned
    /// source must be `Package`-scoped with `local_root()` pointing to the
    /// ancestor `.creft/`.
    #[test]
    fn resolve_command_package_skill_falls_through_empty_intermediate_creft() {
        use pretty_assertions::assert_eq;

        let home_dir = tempfile::TempDir::new().unwrap();
        let project_dir = tempfile::TempDir::new().unwrap();
        let sub_dir = project_dir.path().join("sub");
        std::fs::create_dir_all(&sub_dir).unwrap();

        // Ancestor root: package installed here.
        let ancestor_root = project_dir.path().join(".creft");
        let pkg_dir = ancestor_root.join("packages").join("mypkg");
        std::fs::create_dir_all(&pkg_dir).unwrap();
        std::fs::write(
            pkg_dir.join("deploy.md"),
            "---\nname: mypkg deploy\ndescription: deploy via package\n---\n\n```bash\necho deploy\n```\n",
        )
        .unwrap();

        // Intermediate root: empty — no packages or commands.
        let sub_root = sub_dir.join(".creft");
        std::fs::create_dir_all(sub_root.join("commands")).unwrap();

        // CWD is inside the sub-project — chain: sub_root (empty) → ancestor_root.
        let ctx = AppContext::for_test(home_dir.path().to_path_buf(), sub_dir.clone());

        let args: Vec<String> = vec!["mypkg".to_string(), "deploy".to_string()];
        let (name, remaining, source) = resolve_command(&ctx, &args)
            .expect("resolve_command must resolve package skill through empty intermediate root");

        assert_eq!(name, "mypkg deploy");
        assert!(remaining.is_empty());
        assert_eq!(
            source.scope(),
            Scope::Local,
            "package skill from ancestor root must be Local-scoped"
        );
        assert_eq!(
            source.local_root(),
            Some(ancestor_root.as_path()),
            "local_root must be the ancestor .creft/, not the intermediate"
        );
    }

    /// A plugin activated in an ancestor `.creft/plugins/settings.json` produces
    /// its commands when `list_all_with_source` is called from a descendant CWD
    /// with its own empty `.creft/`. Plugin sources have `local_root() == None`.
    #[test]
    fn list_all_with_source_includes_ancestor_activated_plugin_from_sub_cwd() {
        use pretty_assertions::assert_eq;

        let home_dir = tempfile::TempDir::new().unwrap();
        let project_dir = tempfile::TempDir::new().unwrap();
        let sub_dir = project_dir.path().join("sub");
        std::fs::create_dir_all(&sub_dir).unwrap();

        // Install the plugin skill into the global plugin cache (~/.creft/plugins/myplugin/).
        let global_root = home_dir.path().join(".creft");
        let plugin_dir = global_root.join("plugins").join("myplugin");
        std::fs::create_dir_all(&plugin_dir).unwrap();
        std::fs::write(
            plugin_dir.join("greet.md"),
            "---\nname: greet\ndescription: plugin greet\n---\n\n```bash\necho hi\n```\n",
        )
        .unwrap();

        // Activate the plugin in the ancestor root's local settings.
        let ancestor_root = project_dir.path().join(".creft");
        let ancestor_plugin_settings_dir = ancestor_root.join("plugins");
        std::fs::create_dir_all(&ancestor_plugin_settings_dir).unwrap();
        std::fs::write(
            ancestor_plugin_settings_dir.join("settings.json"),
            r#"{"activated":{"myplugin":true}}"#,
        )
        .unwrap();

        // Descendant root: exists but has no activations.
        let sub_root = sub_dir.join(".creft");
        std::fs::create_dir_all(&sub_root).unwrap();

        // CWD is the descendant — chain: sub_root → ancestor_root.
        let ctx = AppContext::for_test(home_dir.path().to_path_buf(), sub_dir.clone());

        let items = list_all_with_source(&ctx).expect("list_all_with_source must succeed");
        let by_name: std::collections::HashMap<&str, &SkillSource> =
            items.iter().map(|(d, s)| (d.name.as_str(), s)).collect();

        assert!(
            by_name.contains_key("greet"),
            "plugin command from ancestor activation must appear in list; got: {:?}",
            by_name.keys().collect::<Vec<_>>()
        );

        let plugin_source = by_name["greet"];
        assert_eq!(
            plugin_source.local_root(),
            None,
            "plugin source must have local_root() == None (plugins are global)"
        );
        assert!(
            matches!(plugin_source, SkillSource::Plugin(name) if name == "myplugin"),
            "source must be Plugin(\"myplugin\"), got: {plugin_source:?}"
        );
    }

    /// Single-root project: `rebuild_all_indexes` produces the same set of
    /// index files as before Stage 2 (regression guard).
    #[test]
    fn rebuild_all_indexes_single_root_unchanged() {
        use crate::search::store as search_store;
        use crate::search::store::index_path;

        let home_dir = tempfile::TempDir::new().unwrap();
        let project_dir = tempfile::TempDir::new().unwrap();
        let local_root = project_dir.path().join(".creft");
        write_skill_dir(&local_root, "deploy rollback", "Roll back a deploy");

        let ctx = AppContext::for_test(
            home_dir.path().to_path_buf(),
            project_dir.path().to_path_buf(),
        );

        search_store::rebuild_all_indexes(&ctx).expect("rebuild_all_indexes must succeed");

        let idx_path = index_path(
            &ctx,
            "deploy",
            Scope::Local,
            None,
            Some(local_root.as_path()),
        )
        .unwrap();
        assert!(
            idx_path.exists(),
            "single-root project must produce a local deploy.idx"
        );
    }
}
