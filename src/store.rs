use std::path::{Path, PathBuf};

use crate::error::CreftError;
use crate::frontmatter;
use crate::markdown;
pub use crate::model::find_local_root_from;
use crate::model::{AppContext, CommandDef, NamespaceEntry, ParsedCommand, Scope, SkillSource};
use crate::registry;

const RESERVED: &[&str] = &[
    "add",
    "list",
    "show",
    "edit",
    "rm",
    "cat",
    "up",
    "help",
    "version",
    "install",
    "update",
    "uninstall",
    "init",
    "doctor",
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

/// Walk up from `start`'s parent directory looking for `.creft/`.
///
/// Skips `start` itself -- only checks ancestors. Returns `None` if no
/// ancestor has a `.creft/` directory.
pub(crate) fn find_parent_local_root(start: &Path) -> Option<PathBuf> {
    let mut dir = start.to_path_buf();
    if !dir.pop() {
        return None;
    }
    find_local_root_from(&dir)
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

    Ok(def.name)
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
            SkillSource::Package(name, _) => {
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
            SkillSource::Owned(_) => {
                // Contains an owned skill -- not a pure package namespace.
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
/// - `SkillSource::Owned(scope)` -> reads from the given scope's commands directory.
/// - `SkillSource::Package(_, scope)` -> reads the raw `.md` file directly from the package
///   directory, preserving all content including code blocks.
pub fn read_raw_from(
    ctx: &AppContext,
    name: &str,
    source: &SkillSource,
) -> Result<String, CreftError> {
    match source {
        SkillSource::Owned(scope) => read_raw_in(ctx, name, *scope),
        SkillSource::Package(_, _) => {
            // Read the file directly rather than parse-and-reserialize, which would
            // drop code block contents.
            let file_path = registry::skill_file_path(ctx, name)?;
            Ok(std::fs::read_to_string(&file_path)?)
        }
    }
}

/// Load and parse a command by name and source.
///
/// - `SkillSource::Owned(scope)` -> reads from the given scope's commands directory.
/// - `SkillSource::Package(_, scope)` -> delegates to `registry::load_package_skill`.
pub fn load_from(
    ctx: &AppContext,
    name: &str,
    source: &SkillSource,
) -> Result<ParsedCommand, CreftError> {
    match source {
        SkillSource::Owned(scope) => load_in(ctx, name, *scope),
        SkillSource::Package(_, _) => registry::load_package_skill(ctx, name),
    }
}

/// List all skills in a single scope along with their source, including package skills.
///
/// Skills in `scope`'s commands directory are returned first, followed by package skills
/// from that scope's packages directory.
fn list_scope_with_packages(
    ctx: &AppContext,
    scope: Scope,
) -> Result<Vec<(CommandDef, SkillSource)>, CreftError> {
    let owned = list_all_in(ctx, scope)?;
    let owned_names: std::collections::HashSet<String> =
        owned.iter().map(|d| d.name.clone()).collect();

    let mut result: Vec<(CommandDef, SkillSource)> = owned
        .into_iter()
        .map(|d| (d, SkillSource::Owned(scope)))
        .collect();

    let packages = registry::list_packages_in(ctx, scope)?;
    for pkg in packages {
        match registry::list_package_skills_in(ctx, &pkg.manifest.name, scope) {
            Ok(skills) => {
                for skill in skills {
                    if !owned_names.contains(&skill.name) {
                        result.push((
                            skill,
                            SkillSource::Package(pkg.manifest.name.clone(), scope),
                        ));
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

/// List all available skills (owned + installed), sorted by name.
///
/// When `creft_home` is set, lists only from that single location.
/// Otherwise, lists from both local and global scopes; local shadows global on name collision.
pub fn list_all_with_source(
    ctx: &AppContext,
) -> Result<Vec<(CommandDef, SkillSource)>, CreftError> {
    if ctx.creft_home.is_some() {
        let mut result = list_scope_with_packages(ctx, Scope::Global)?;
        result.sort_by(|a, b| a.0.name.cmp(&b.0.name));
        return Ok(result);
    }

    let mut result = Vec::new();
    let mut seen_names: std::collections::HashSet<String> = std::collections::HashSet::new();

    if ctx.find_local_root().is_some() {
        for item in list_scope_with_packages(ctx, Scope::Local)? {
            seen_names.insert(item.0.name.clone());
            result.push(item);
        }
    }

    for item in list_scope_with_packages(ctx, Scope::Global)? {
        if !seen_names.contains(&item.0.name) {
            result.push(item);
        }
    }

    result.sort_by(|a, b| a.0.name.cmp(&b.0.name));
    Ok(result)
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

/// Resolve a command within a single scope (owned commands, then packages).
///
/// Returns `(command_name, remaining_args, SkillSource)` or `CreftError::CommandNotFound`.
fn resolve_in_single_scope(
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
            return Ok((candidate, args[len..].to_vec(), SkillSource::Owned(scope)));
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
                return Ok((candidate, args[len..].to_vec(), SkillSource::Owned(scope)));
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
                return Ok((
                    full_name,
                    extra_args,
                    SkillSource::Package(first.to_string(), scope),
                ));
            }
        }
        if args.len() > 1 {
            return Err(CreftError::CommandNotFound(args[..2].join(" ")));
        }
    }

    Err(CreftError::CommandNotFound(args[0].clone()))
}

/// Returns true if the source is local-scope (owned or package).
pub(crate) fn is_local_source(source: &SkillSource) -> bool {
    matches!(
        source,
        SkillSource::Owned(Scope::Local) | SkillSource::Package(_, Scope::Local)
    )
}

/// Resolve a command name from raw CLI args.
///
/// Resolution order:
/// 1. If `creft_home` is set: use only that single location.
/// 2. Try local scope (owned commands, then packages).
/// 3. Try global scope (owned commands, then packages).
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
        return resolve_in_single_scope(ctx, args, Scope::Global);
    }

    if ctx.find_local_root().is_some()
        && let Ok(result) = resolve_in_single_scope(ctx, args, Scope::Local)
    {
        return Ok(result);
    }

    resolve_in_single_scope(ctx, args, Scope::Global)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[allow(unused_imports)]
    use pretty_assertions::{assert_eq, assert_ne};

    #[test]
    fn test_is_reserved() {
        assert!(is_reserved("add"));
        assert!(is_reserved("list"));
        assert!(!is_reserved("hello"));
        assert!(!is_reserved("gh"));
    }

    #[test]
    fn test_validate_name_ok() {
        assert!(validate_name("hello").is_ok());
        assert!(validate_name("gh issue-body").is_ok());
        assert!(validate_name("my_cmd").is_ok());
    }

    #[test]
    fn test_validate_name_reserved() {
        assert!(matches!(
            validate_name("add"),
            Err(CreftError::ReservedName(_))
        ));
        assert!(matches!(
            validate_name("list"),
            Err(CreftError::ReservedName(_))
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
        assert!(is_local_source(&SkillSource::Owned(Scope::Local)));
    }

    #[test]
    fn test_is_local_source_owned_global() {
        assert!(!is_local_source(&SkillSource::Owned(Scope::Global)));
    }

    #[test]
    fn test_is_local_source_package_local() {
        assert!(is_local_source(&SkillSource::Package(
            "mypkg".to_string(),
            Scope::Local
        )));
    }

    #[test]
    fn test_is_local_source_package_global() {
        assert!(!is_local_source(&SkillSource::Package(
            "mypkg".to_string(),
            Scope::Global
        )));
    }

    // --- validate_path_token unit tests ---

    #[test]
    fn test_validate_path_token_rejects_dot() {
        assert!(matches!(
            validate_path_token("."),
            Err(CreftError::InvalidName(_))
        ));
    }

    #[test]
    fn test_validate_path_token_rejects_dotdot() {
        assert!(matches!(
            validate_path_token(".."),
            Err(CreftError::InvalidName(_))
        ));
    }

    #[test]
    fn test_validate_path_token_rejects_slash() {
        assert!(matches!(
            validate_path_token("a/b"),
            Err(CreftError::InvalidName(_))
        ));
    }

    #[test]
    fn test_validate_path_token_rejects_backslash() {
        assert!(matches!(
            validate_path_token("a\\b"),
            Err(CreftError::InvalidName(_))
        ));
    }

    #[test]
    fn test_validate_path_token_rejects_empty() {
        assert!(matches!(
            validate_path_token(""),
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

    // --- find_local_root_from tests ---

    #[test]
    fn test_find_local_root_from_finds_creft_at_start() {
        // .creft/ exists directly in the start directory — should be returned immediately.
        let dir = tempfile::TempDir::new().unwrap();
        let creft_dir = dir.path().join(".creft");
        std::fs::create_dir_all(&creft_dir).unwrap();

        let result = find_local_root_from(dir.path());
        assert_eq!(result, Some(creft_dir));
    }

    #[test]
    fn test_find_local_root_from_finds_creft_in_parent() {
        // .creft/ exists in the parent of start — walk-up must find it.
        let parent = tempfile::TempDir::new().unwrap();
        let creft_dir = parent.path().join(".creft");
        std::fs::create_dir_all(&creft_dir).unwrap();

        // Child directory inside the parent (no .creft/ of its own).
        let child = parent.path().join("subdir");
        std::fs::create_dir_all(&child).unwrap();

        let result = find_local_root_from(&child);
        assert_eq!(result, Some(creft_dir));
    }

    #[test]
    fn test_find_local_root_from_returns_none_when_absent() {
        // No .creft/ anywhere in the temp dir tree — must return None.
        // TempDir is created under /tmp which is outside any real .creft/ tree.
        let dir = tempfile::TempDir::new().unwrap();
        let child = dir.path().join("a").join("b").join("c");
        std::fs::create_dir_all(&child).unwrap();

        let result = find_local_root_from(&child);
        assert!(
            result.is_none(),
            "expected None when no .creft/ exists, got {:?}",
            result
        );
    }

    #[test]
    fn test_find_local_root_from_skips_creft_file() {
        // .creft exists as a file (not a directory) — must be skipped, returning None.
        let dir = tempfile::TempDir::new().unwrap();
        let creft_file = dir.path().join(".creft");
        std::fs::write(&creft_file, "not a directory").unwrap();

        let result = find_local_root_from(dir.path());
        assert!(
            result.is_none(),
            "expected None when .creft is a file, got {:?}",
            result
        );
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

    // --- scope-aware resolution tests ---
    //
    // These tests exercise local/global split WITHOUT CREFT_HOME so that the two-tier
    // resolution path is exercised. They use AppContext::for_test() with explicit temp
    // directories — no env vars or CWD mutation needed.

    fn make_skill(name: &str, desc: &str) -> String {
        format!("---\nname: {name}\ndescription: {desc}\n---\n\n```bash\necho {name}\n```\n")
    }

    /// Writes a skill file to `<root>/commands/<name>.md`, creating directories as needed.
    fn write_skill_to_root(root: &std::path::Path, name: &str, desc: &str) {
        let path = root.join("commands").join(format!("{}.md", name));
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
            by_name.get("hello"),
            Some(&&SkillSource::Owned(Scope::Local)),
            "local hello must shadow global hello"
        );
        assert_eq!(
            by_name.get("local-only"),
            Some(&&SkillSource::Owned(Scope::Local)),
            "local-only must appear with Local scope"
        );
        assert_eq!(
            by_name.get("global-only"),
            Some(&&SkillSource::Owned(Scope::Global)),
            "global-only must appear with Global scope"
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

    // --- find_parent_local_root unit tests ---

    #[test]
    fn test_find_parent_local_root_found() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join(".creft")).unwrap();
        let child = tmp.path().join("child");
        std::fs::create_dir(&child).unwrap();
        let result = find_parent_local_root(&child);
        assert!(result.is_some());
        assert_eq!(result.unwrap(), tmp.path().join(".creft"));
    }

    #[test]
    fn test_find_parent_local_root_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        let child = tmp.path().join("child");
        std::fs::create_dir(&child).unwrap();
        // No .creft/ anywhere in the tempdir hierarchy
        assert!(find_parent_local_root(&child).is_none());
    }

    #[test]
    fn test_find_parent_local_root_skips_start() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join(".creft")).unwrap();
        // Start is the dir that HAS .creft/ -- should not find it
        assert!(find_parent_local_root(tmp.path()).is_none());
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
        (def, SkillSource::Owned(Scope::Global))
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
        (def, SkillSource::Package(pkg.to_string(), Scope::Global))
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
}
