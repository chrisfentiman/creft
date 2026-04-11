use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::CreftError;
use crate::frontmatter;
use crate::markdown;
use crate::model::{AppContext, CommandDef, ParsedCommand, Scope};
use crate::store;

/// Manifest from a skill package's `creft.yaml` file.
#[derive(Debug, Clone, Deserialize)]
pub struct PackageManifest {
    pub name: String,
    pub version: String,
    #[allow(dead_code)] // deserialized from manifest
    pub description: String,
    #[serde(default)]
    #[allow(dead_code)] // deserialized from manifest
    pub author: Option<String>,
    #[serde(default)]
    #[allow(dead_code)] // deserialized from manifest
    pub license: Option<String>,
}

/// Metadata about an installed package, derived from manifest + filesystem.
#[derive(Debug, Clone)]
pub struct InstalledPackage {
    pub manifest: PackageManifest,
    #[allow(dead_code)] // set from filesystem at install time; read only in tests
    pub path: PathBuf,
}

/// Validate a manifest name.
///
/// Rules (applied in order):
/// 1. Must be non-empty.
/// 2. Must contain NO whitespace — manifest names are single contiguous tokens.
/// 3. Every character must be alphanumeric, `-`, or `_`.
/// 4. Must not be a reserved built-in name.
pub fn validate_manifest_name(name: &str) -> Result<(), CreftError> {
    if name.is_empty() {
        return Err(CreftError::InvalidManifest(
            "package name cannot be empty".into(),
        ));
    }

    // Whitespace check BEFORE character validation — a name with spaces is not
    // a single token and must be rejected before store::validate_name splits on whitespace.
    if name.contains(char::is_whitespace) {
        return Err(CreftError::InvalidManifest(format!(
            "package name '{name}' must not contain whitespace"
        )));
    }

    // Delegate character + reserved-name validation to store::validate_name.
    // At this point `name` has no whitespace, so validate_name treats it as a
    // single-part name.
    store::validate_name(name).map_err(|e| CreftError::InvalidManifest(e.to_string()))
}

/// Move a directory from `src` to `dst`.
///
/// Tries `std::fs::rename` first (fast, atomic on same filesystem).
/// Falls back to a recursive copy + delete when rename fails across filesystems.
/// On failure of the copy, cleans up both the partial `dst` and the `src` before returning.
pub(crate) fn move_dir(src: &Path, dst: &Path) -> Result<(), CreftError> {
    match std::fs::rename(src, dst) {
        Ok(()) => Ok(()),
        Err(_) => {
            // Cross-filesystem move: copy then delete.
            if let Err(e) = copy_dir_recursive(src, dst) {
                // On failure, attempt to clean up the partial copy and return the error.
                let _ = std::fs::remove_dir_all(dst);
                let _ = std::fs::remove_dir_all(src);
                return Err(e);
            }
            // Copy succeeded; clean up the source.
            let _ = std::fs::remove_dir_all(src);
            Ok(())
        }
    }
}

/// Recursively copy a directory tree from `src` to `dst`.
fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), CreftError> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let file_type = entry.file_type()?;

        // Skip symlinks -- a malicious package could use symlinks to
        // exfiltrate files from outside the package directory.
        if file_type.is_symlink() {
            continue;
        }

        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            std::fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}

/// List all installed packages in the given scope.
///
/// Reads `ctx.packages_dir_for(scope)` and parses `creft.yaml` for each subdirectory.
/// Entries that fail to parse are skipped with a warning to stderr.
pub fn list_packages_in(
    ctx: &AppContext,
    scope: Scope,
) -> Result<Vec<InstalledPackage>, CreftError> {
    let base = ctx.packages_dir_for(scope)?;
    if !base.exists() {
        return Ok(Vec::new());
    }

    let mut packages = Vec::new();
    let entries = std::fs::read_dir(&base)?;
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let manifest_path = path.join("creft.yaml");
        match std::fs::read_to_string(&manifest_path) {
            Ok(content) => match serde_yaml_ng::from_str::<PackageManifest>(&content) {
                Ok(manifest) => packages.push(InstalledPackage { manifest, path }),
                Err(e) => eprintln!(
                    "warning: skipping {}: invalid manifest: {}",
                    path.display(),
                    e
                ),
            },
            Err(e) => eprintln!(
                "warning: skipping {}: could not read creft.yaml: {}",
                path.display(),
                e
            ),
        }
    }

    packages.sort_by(|a, b| a.manifest.name.cmp(&b.manifest.name));
    Ok(packages)
}

/// List all skills in an installed package in the given scope.
///
/// Walks the package directory recursively for `.md` files.
/// Skill names are derived from file paths relative to the package root —
/// frontmatter `name` fields are ignored.
///
/// Skips `creft.yaml`, dotfiles, and dot-directories.
/// Enforces a 3-level nesting cap: files nested more than 3 directory levels
/// deep within the package root are skipped (to match CLI resolution limits).
pub fn list_package_skills_in(
    ctx: &AppContext,
    name: &str,
    scope: Scope,
) -> Result<Vec<CommandDef>, CreftError> {
    let pkg_dir = ctx.packages_dir_for(scope)?.join(name);
    if !pkg_dir.exists() {
        return Err(CreftError::PackageNotFound(name.to_string()));
    }

    let mut skills = Vec::new();
    collect_package_skills(name, &pkg_dir, &pkg_dir, 0, &mut skills)?;
    skills.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(skills)
}

/// Recursively collect `.md` skills from a package directory.
///
/// `depth` is the current directory nesting level relative to the package root.
/// Files at depth > 3 (i.e. more than 3 path components between package root
/// and the file) are skipped.
fn collect_package_skills(
    pkg_name: &str,
    pkg_root: &Path,
    dir: &Path,
    depth: usize,
    skills: &mut Vec<CommandDef>,
) -> Result<(), CreftError> {
    let entries = std::fs::read_dir(dir)?;
    for entry in entries {
        let entry = entry?;
        let file_type = entry.file_type()?;

        // Skip symlinks -- a malicious package could use symlinks to
        // traverse outside the package directory.
        if file_type.is_symlink() {
            continue;
        }

        let path = entry.path();

        // Skip dotfiles and dot-directories
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        if file_name.starts_with('.') {
            continue;
        }

        if file_type.is_dir() {
            // depth is the current directory level; files inside are at depth+1.
            // Cap is 3 levels, so recurse only when depth < 3.
            if depth < 3 {
                collect_package_skills(pkg_name, pkg_root, &path, depth + 1, skills)?;
            }
            continue;
        }

        if path.extension().is_none_or(|e| e != "md") {
            continue;
        }

        let rel = path
            .strip_prefix(pkg_root)
            .expect("path must be under pkg_root");
        let skill_name = skill_name_from_path(pkg_name, rel);

        match std::fs::read_to_string(&path) {
            Ok(content) => match frontmatter::parse(&content) {
                Ok((mut def, _)) => {
                    def.name = skill_name;
                    skills.push(def);
                }
                Err(e) => eprintln!("warning: skipping {}: {}", path.display(), e),
            },
            Err(e) => eprintln!("warning: skipping {}: {}", path.display(), e),
        }
    }
    Ok(())
}

/// Compute a namespaced skill name from a package name and a relative file path.
///
/// Directory separators become spaces and the `.md` extension is stripped.
///
/// Examples:
/// - `"pkg"`, `"deploy.md"` -> `"pkg deploy"`
/// - `"k8s-tools"`, `"networking/check-dns.md"` -> `"k8s-tools networking check-dns"`
fn skill_name_from_path(pkg_name: &str, rel: &Path) -> String {
    let mut parts = vec![pkg_name.to_string()];
    if let Some(parent) = rel.parent() {
        for component in parent.components() {
            parts.push(component.as_os_str().to_string_lossy().into_owned());
        }
    }
    if let Some(stem) = rel.file_stem().and_then(|s| s.to_str()) {
        parts.push(stem.to_string());
    }
    parts.join(" ")
}

/// Build the file path for a package skill within a specific scope.
///
/// Returns `None` if the path does not exist in that scope.
fn skill_file_path_in(
    ctx: &AppContext,
    pkg_name: &str,
    rel_parts: &[&str],
    scope: Scope,
) -> Result<Option<PathBuf>, CreftError> {
    let pkg_dir = ctx.packages_dir_for(scope)?.join(pkg_name);
    if !pkg_dir.exists() {
        return Ok(None);
    }
    let mut file_path = pkg_dir;
    for (i, part) in rel_parts.iter().enumerate() {
        if i == rel_parts.len() - 1 {
            file_path = file_path.join(format!("{}.md", part));
        } else {
            file_path = file_path.join(part);
        }
    }
    if file_path.exists() {
        Ok(Some(file_path))
    } else {
        Ok(None)
    }
}

/// Resolve the filesystem path for a fully-qualified package skill name.
///
/// The `full_name` must contain at least two whitespace-separated tokens:
/// - Token 0 is the package name (maps to `ctx.packages_dir_for()/<name>/`).
/// - Tokens 1..N are path components; the last token gets a `.md` suffix.
///
/// When `creft_home` is set, only the single location is checked.
/// Otherwise, local scope is checked before global scope.
///
/// Returns an error if the package directory does not exist or the skill file
/// is not found.
pub fn skill_file_path(ctx: &AppContext, full_name: &str) -> Result<PathBuf, CreftError> {
    let tokens: Vec<&str> = full_name.split_whitespace().collect();
    if tokens.len() < 2 {
        return Err(CreftError::PackageNotFound(full_name.to_string()));
    }

    let pkg_name = tokens[0];
    let rel_parts = &tokens[1..];

    // Validate each token for path traversal before constructing any path.
    for part in rel_parts {
        store::validate_path_token(part)?;
    }

    // CREFT_HOME mode: single scope only.
    if ctx.creft_home.is_some() {
        return skill_file_path_in(ctx, pkg_name, rel_parts, Scope::Global)?
            .ok_or_else(|| CreftError::PackageNotFound(full_name.to_string()));
    }

    // Check local scope first.
    if ctx.find_local_root().is_some()
        && let Some(path) = skill_file_path_in(ctx, pkg_name, rel_parts, Scope::Local)?
    {
        return Ok(path);
    }

    // Fall back to global scope.
    skill_file_path_in(ctx, pkg_name, rel_parts, Scope::Global)?
        .ok_or_else(|| CreftError::PackageNotFound(full_name.to_string()))
}

/// Load and parse a skill from an installed package by its full namespaced name.
pub fn load_package_skill(ctx: &AppContext, full_name: &str) -> Result<ParsedCommand, CreftError> {
    let file_path = skill_file_path(ctx, full_name)?;

    let content = std::fs::read_to_string(&file_path)?;
    let (mut def, body) = frontmatter::parse(&content)?;
    let (docs, blocks) = markdown::extract_blocks(&body);

    // The frontmatter name is relative to the package; replace it with the
    // fully-qualified namespaced name so callers get a consistent identifier.
    def.name = full_name.to_string();

    Ok(ParsedCommand { def, docs, blocks })
}

/// Install a plugin from a git URL into the global plugin cache.
///
/// 1. Clone the repo to a temp directory with `git clone --depth 1`.
/// 2. Read and validate `creft.yaml`.
/// 3. Check the global plugins directory for an existing plugin with the same name.
/// 4. Move the clone to `ctx.plugins_dir()/<name>/`.
///
/// The `plugin_filter` parameter is reserved for multi-plugin repos and is not
/// yet implemented. Passing a value returns an error.
pub fn plugin_install(
    ctx: &AppContext,
    url: &str,
    plugin_filter: Option<&str>,
) -> Result<InstalledPackage, CreftError> {
    if plugin_filter.is_some() {
        return Err(CreftError::Setup(
            "--plugin selection for multi-plugin repos is not yet implemented".into(),
        ));
    }

    let tmp = tempfile::TempDir::new()?;
    let tmp_path = tmp.path().to_path_buf();

    // The `--` separator prevents a user-supplied URL starting with `-`
    // from being interpreted as a git flag.
    let output = std::process::Command::new("git")
        .args([
            "clone",
            "--depth",
            "1",
            "--",
            url,
            tmp_path.to_str().unwrap_or_default(),
        ])
        .output()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                CreftError::Git(
                    "git is not installed. Install git to use plugin management.".into(),
                )
            } else {
                CreftError::Git(e.to_string())
            }
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(CreftError::Git(stderr));
    }

    let manifest_path = tmp_path.join("creft.yaml");
    if !manifest_path.exists() {
        return Err(CreftError::ManifestNotFound);
    }

    let manifest_content = std::fs::read_to_string(&manifest_path)?;
    let manifest: PackageManifest = serde_yaml_ng::from_str(&manifest_content)
        .map_err(|e| CreftError::InvalidManifest(e.to_string()))?;

    validate_manifest_name(&manifest.name)?;

    let plugins = ctx.plugins_dir()?;
    let dest = plugins.join(&manifest.name);
    if dest.exists() {
        return Err(CreftError::PackageAlreadyInstalled(manifest.name.clone()));
    }

    if !plugins.exists() {
        std::fs::create_dir_all(&plugins)?;
    }

    move_dir(&tmp_path, &dest)?;
    let _ = tmp.keep();

    Ok(InstalledPackage {
        manifest,
        path: dest,
    })
}

/// Update an installed plugin by running `git pull --ff-only` in its directory.
pub fn plugin_update(ctx: &AppContext, name: &str) -> Result<InstalledPackage, CreftError> {
    let plugin_path = ctx.plugins_dir()?.join(name);
    if !plugin_path.exists() {
        return Err(CreftError::PackageNotFound(name.to_string()));
    }

    let output = std::process::Command::new("git")
        .args([
            "-C",
            plugin_path.to_str().unwrap_or_default(),
            "pull",
            "--ff-only",
        ])
        .output()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                CreftError::Git(
                    "git is not installed. Install git to use plugin management.".into(),
                )
            } else {
                CreftError::Git(e.to_string())
            }
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(CreftError::Git(stderr));
    }

    let manifest_path = plugin_path.join("creft.yaml");
    if !manifest_path.exists() {
        return Err(CreftError::ManifestNotFound);
    }

    let manifest_content = std::fs::read_to_string(&manifest_path)?;
    let manifest: PackageManifest = serde_yaml_ng::from_str(&manifest_content)
        .map_err(|e| CreftError::InvalidManifest(e.to_string()))?;

    validate_manifest_name(&manifest.name)?;

    if manifest.name != name {
        return Err(CreftError::InvalidManifest(format!(
            "plugin name changed from '{}' to '{}' -- uninstall and reinstall",
            name, manifest.name
        )));
    }

    Ok(InstalledPackage {
        manifest,
        path: plugin_path,
    })
}

/// Update all installed plugins. Returns results for each.
///
/// Individual update failures are collected — a failure for one plugin does not
/// stop the others from being attempted.
pub fn plugin_update_all(
    ctx: &AppContext,
) -> Result<Vec<Result<InstalledPackage, CreftError>>, CreftError> {
    let base = ctx.plugins_dir()?;
    if !base.exists() {
        return Ok(Vec::new());
    }

    let mut results = Vec::new();
    for entry in std::fs::read_dir(&base)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            results.push(plugin_update(ctx, name));
        }
    }
    Ok(results)
}

/// Remove an installed plugin by deleting its directory from the global cache.
pub fn plugin_uninstall(ctx: &AppContext, name: &str) -> Result<(), CreftError> {
    let plugin_path = ctx.plugins_dir()?.join(name);
    if !plugin_path.exists() {
        return Err(CreftError::PackageNotFound(name.to_string()));
    }
    std::fs::remove_dir_all(&plugin_path)?;
    Ok(())
}

/// List all skill definitions in an installed plugin.
///
/// Walks the plugin directory recursively for `.md` files. Skill names are
/// derived from file paths relative to the plugin root. Dotfiles, symlinks,
/// and files nested more than 3 levels deep are skipped.
pub fn list_plugin_skills_in(ctx: &AppContext, name: &str) -> Result<Vec<CommandDef>, CreftError> {
    let plugin_dir = ctx.plugins_dir()?.join(name);
    if !plugin_dir.exists() {
        return Err(CreftError::PackageNotFound(name.to_string()));
    }

    let mut skills = Vec::new();
    collect_package_skills(name, &plugin_dir, &plugin_dir, 0, &mut skills)?;
    skills.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(skills)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use rstest::rstest;

    // --- PackageManifest deserialization ---

    #[test]
    fn test_manifest_full_fields() {
        let yaml = "name: k8s-tools\nversion: 0.1.0\ndescription: Kubernetes skills\nauthor: someone\nlicense: MIT\n";
        let manifest: PackageManifest = serde_yaml_ng::from_str(yaml).unwrap();
        assert_eq!(manifest.name, "k8s-tools");
        assert_eq!(manifest.version, "0.1.0");
        assert_eq!(manifest.description, "Kubernetes skills");
        assert_eq!(manifest.author, Some("someone".into()));
        assert_eq!(manifest.license, Some("MIT".into()));
    }

    #[test]
    fn test_manifest_optional_fields_absent() {
        let yaml = "name: my-pkg\nversion: 1.0.0\ndescription: A package\n";
        let manifest: PackageManifest = serde_yaml_ng::from_str(yaml).unwrap();
        assert_eq!(manifest.name, "my-pkg");
        assert!(manifest.author.is_none());
        assert!(manifest.license.is_none());
    }

    #[test]
    fn test_manifest_missing_required_field_name() {
        let yaml = "version: 1.0.0\ndescription: A package\n";
        let result: Result<PackageManifest, _> = serde_yaml_ng::from_str(yaml);
        assert!(result.is_err());
    }

    #[test]
    fn test_manifest_missing_required_field_version() {
        let yaml = "name: my-pkg\ndescription: A package\n";
        let result: Result<PackageManifest, _> = serde_yaml_ng::from_str(yaml);
        assert!(result.is_err());
    }

    #[test]
    fn test_manifest_missing_required_field_description() {
        let yaml = "name: my-pkg\nversion: 1.0.0\n";
        let result: Result<PackageManifest, _> = serde_yaml_ng::from_str(yaml);
        assert!(result.is_err());
    }

    // --- validate_manifest_name ---

    #[test]
    fn test_validate_manifest_name_ok() {
        assert!(validate_manifest_name("k8s-tools").is_ok());
        assert!(validate_manifest_name("my_package").is_ok());
        assert!(validate_manifest_name("pkg123").is_ok());
        assert!(validate_manifest_name("a").is_ok());
    }

    #[test]
    fn test_validate_manifest_name_empty() {
        let err = validate_manifest_name("").unwrap_err();
        assert!(matches!(err, CreftError::InvalidManifest(_)));
        assert!(err.to_string().contains("empty"));
    }

    #[test]
    fn test_validate_manifest_name_whitespace_rejected() {
        // A name with spaces must be rejected before character validation.
        let err = validate_manifest_name("hello world").unwrap_err();
        assert!(matches!(err, CreftError::InvalidManifest(_)));
        assert!(err.to_string().contains("whitespace"));
    }

    #[test]
    fn test_validate_manifest_name_tabs_rejected() {
        let err = validate_manifest_name("hello\tworld").unwrap_err();
        assert!(matches!(err, CreftError::InvalidManifest(_)));
        assert!(err.to_string().contains("whitespace"));
    }

    #[test]
    fn test_validate_manifest_name_invalid_chars() {
        let err = validate_manifest_name("pkg!name").unwrap_err();
        assert!(matches!(err, CreftError::InvalidManifest(_)));
    }

    #[test]
    fn test_validate_manifest_name_semicolon_rejected() {
        let err = validate_manifest_name("pkg;rm").unwrap_err();
        assert!(matches!(err, CreftError::InvalidManifest(_)));
    }

    #[test]
    fn test_validate_manifest_name_reserved_add() {
        let err = validate_manifest_name("add").unwrap_err();
        assert!(matches!(err, CreftError::InvalidManifest(_)));
    }

    /// `install`, `update`, and `uninstall` are no longer reserved names.
    /// Plugin management moved under `creft plugin`, freeing these names for skill authors.
    #[rstest]
    #[case::install("install")]
    #[case::update("update")]
    #[case::uninstall("uninstall")]
    fn formerly_reserved_names_are_now_valid(#[case] name: &str) {
        assert!(validate_manifest_name(name).is_ok());
    }

    #[test]
    fn test_validate_manifest_name_plugin_is_reserved() {
        // `plugin` is the new reserved name for the plugin management namespace.
        let err = validate_manifest_name("plugin").unwrap_err();
        assert!(matches!(err, CreftError::InvalidManifest(_)));
    }

    // --- packages_dir (via AppContext) ---

    #[test]
    fn test_packages_dir_uses_creft_home() {
        let ctx = AppContext::for_test_with_creft_home(
            std::path::PathBuf::from("/tmp/creft-test-pkgs"),
            std::path::PathBuf::from("/tmp/creft-test-pkgs"),
        );
        let dir = ctx.packages_dir_for(Scope::Global).unwrap();
        assert_eq!(
            dir,
            std::path::PathBuf::from("/tmp/creft-test-pkgs/packages")
        );
    }

    #[test]
    fn test_packages_dir_different_from_commands_dir() {
        let ctx = AppContext::for_test_with_creft_home(
            std::path::PathBuf::from("/tmp/creft-test-pkgs"),
            std::path::PathBuf::from("/tmp/creft-test-pkgs"),
        );
        let pkgs = ctx.packages_dir_for(Scope::Global).unwrap();
        let cmds = ctx.commands_dir_for(Scope::Global).unwrap();
        assert_ne!(pkgs, cmds);
        assert_eq!(pkgs.file_name().unwrap(), "packages");
        assert_eq!(cmds.file_name().unwrap(), "commands");
    }

    // --- skill_name_from_path ---

    #[test]
    fn test_skill_name_from_path_simple() {
        // deploy.md -> "pkg deploy"
        let rel = Path::new("deploy.md");
        assert_eq!(skill_name_from_path("pkg", rel), "pkg deploy");
    }

    #[test]
    fn test_skill_name_from_path_one_level_deep() {
        // networking/check-dns.md -> "pkg networking check-dns"
        let rel = Path::new("networking/check-dns.md");
        assert_eq!(skill_name_from_path("pkg", rel), "pkg networking check-dns");
    }

    #[test]
    fn test_skill_name_from_path_three_levels() {
        // a/b/c.md -> "pkg a b c"
        let rel = Path::new("a/b/c.md");
        assert_eq!(skill_name_from_path("pkg", rel), "pkg a b c");
    }

    #[test]
    fn test_skill_name_from_path_package_name_in_result() {
        let rel = Path::new("deploy.md");
        let name = skill_name_from_path("k8s-tools", rel);
        assert!(name.starts_with("k8s-tools "));
    }

    // --- list_packages ---

    #[test]
    fn test_list_packages_empty_when_no_dir() {
        let dir = tempfile::TempDir::new().unwrap();
        let ctx = AppContext::for_test_with_creft_home(
            dir.path().to_path_buf(),
            dir.path().to_path_buf(),
        );
        let pkgs = list_packages_in(&ctx, Scope::Global).unwrap();
        assert!(pkgs.is_empty());
    }

    #[test]
    fn test_list_packages_returns_installed() {
        let dir = tempfile::TempDir::new().unwrap();

        // Create a package directory with a creft.yaml
        let pkg_dir = dir.path().join("packages").join("my-pkg");
        std::fs::create_dir_all(&pkg_dir).unwrap();
        std::fs::write(
            pkg_dir.join("creft.yaml"),
            "name: my-pkg\nversion: 1.0.0\ndescription: A test package\n",
        )
        .unwrap();

        let ctx = AppContext::for_test_with_creft_home(
            dir.path().to_path_buf(),
            dir.path().to_path_buf(),
        );
        let pkgs = list_packages_in(&ctx, Scope::Global).unwrap();
        assert_eq!(pkgs.len(), 1);
        assert_eq!(pkgs[0].manifest.name, "my-pkg");
        assert_eq!(pkgs[0].path, pkg_dir);
    }

    #[test]
    fn test_list_packages_skips_invalid_manifest() {
        let dir = tempfile::TempDir::new().unwrap();

        // Valid package
        let pkg1 = dir.path().join("packages").join("good-pkg");
        std::fs::create_dir_all(&pkg1).unwrap();
        std::fs::write(
            pkg1.join("creft.yaml"),
            "name: good-pkg\nversion: 1.0.0\ndescription: Good\n",
        )
        .unwrap();

        // Invalid package — missing required fields
        let pkg2 = dir.path().join("packages").join("bad-pkg");
        std::fs::create_dir_all(&pkg2).unwrap();
        std::fs::write(pkg2.join("creft.yaml"), "not_valid_yaml: [unclosed").unwrap();

        let ctx = AppContext::for_test_with_creft_home(
            dir.path().to_path_buf(),
            dir.path().to_path_buf(),
        );
        let pkgs = list_packages_in(&ctx, Scope::Global).unwrap();
        assert_eq!(pkgs.len(), 1);
        assert_eq!(pkgs[0].manifest.name, "good-pkg");
    }

    // --- list_package_skills ---

    #[test]
    fn test_list_package_skills_not_found() {
        let dir = tempfile::TempDir::new().unwrap();
        let ctx = AppContext::for_test_with_creft_home(
            dir.path().to_path_buf(),
            dir.path().to_path_buf(),
        );

        let result = list_package_skills_in(&ctx, "nonexistent", Scope::Global);
        assert!(matches!(result, Err(CreftError::PackageNotFound(_))));
    }

    #[test]
    fn test_list_package_skills_computes_names_from_paths() {
        let dir = tempfile::TempDir::new().unwrap();

        let pkg_dir = dir.path().join("packages").join("k8s-tools");
        std::fs::create_dir_all(&pkg_dir).unwrap();

        // Top-level skill: deploy.md -> "k8s-tools deploy"
        std::fs::write(
            pkg_dir.join("deploy.md"),
            "---\nname: ignored-name\ndescription: deploy\n---\n\n```bash\necho deploy\n```\n",
        )
        .unwrap();

        // Nested skill: networking/check-dns.md -> "k8s-tools networking check-dns"
        std::fs::create_dir_all(pkg_dir.join("networking")).unwrap();
        std::fs::write(
            pkg_dir.join("networking").join("check-dns.md"),
            "---\nname: ignored\ndescription: check dns\n---\n\n```bash\necho dns\n```\n",
        )
        .unwrap();

        let ctx = AppContext::for_test_with_creft_home(
            dir.path().to_path_buf(),
            dir.path().to_path_buf(),
        );
        let skills = list_package_skills_in(&ctx, "k8s-tools", Scope::Global).unwrap();
        let names: Vec<&str> = skills.iter().map(|s| s.name.as_str()).collect();
        assert!(
            names.contains(&"k8s-tools deploy"),
            "expected k8s-tools deploy, got: {:?}",
            names
        );
        assert!(
            names.contains(&"k8s-tools networking check-dns"),
            "expected k8s-tools networking check-dns, got: {:?}",
            names
        );
    }

    #[test]
    fn test_list_package_skills_excludes_dotfiles() {
        let dir = tempfile::TempDir::new().unwrap();

        let pkg_dir = dir.path().join("packages").join("mypkg");
        std::fs::create_dir_all(&pkg_dir).unwrap();

        // Real skill
        std::fs::write(
            pkg_dir.join("skill.md"),
            "---\nname: skill\ndescription: a skill\n---\n\n```bash\necho ok\n```\n",
        )
        .unwrap();

        // Dotfile — should be excluded
        std::fs::write(
            pkg_dir.join(".hidden.md"),
            "---\nname: hidden\ndescription: hidden\n---\n\n```bash\necho hidden\n```\n",
        )
        .unwrap();

        let ctx = AppContext::for_test_with_creft_home(
            dir.path().to_path_buf(),
            dir.path().to_path_buf(),
        );
        let skills = list_package_skills_in(&ctx, "mypkg", Scope::Global).unwrap();
        let names: Vec<&str> = skills.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(skills.len(), 1, "got: {:?}", names);
        assert_eq!(names[0], "mypkg skill");
    }

    #[test]
    fn test_list_package_skills_excludes_creft_yaml() {
        let dir = tempfile::TempDir::new().unwrap();

        let pkg_dir = dir.path().join("packages").join("mypkg");
        std::fs::create_dir_all(&pkg_dir).unwrap();

        // creft.yaml — not a .md file, should be excluded by extension filter
        std::fs::write(
            pkg_dir.join("creft.yaml"),
            "name: mypkg\nversion: 1.0.0\ndescription: A package\n",
        )
        .unwrap();

        // Real skill
        std::fs::write(
            pkg_dir.join("skill.md"),
            "---\nname: skill\ndescription: a skill\n---\n\n```bash\necho ok\n```\n",
        )
        .unwrap();

        let ctx = AppContext::for_test_with_creft_home(
            dir.path().to_path_buf(),
            dir.path().to_path_buf(),
        );
        let skills = list_package_skills_in(&ctx, "mypkg", Scope::Global).unwrap();
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "mypkg skill");
    }

    #[test]
    fn test_list_package_skills_nesting_cap() {
        let dir = tempfile::TempDir::new().unwrap();

        let pkg_dir = dir.path().join("packages").join("mypkg");

        // Depth 1: a/skill.md -> "mypkg a skill" (allowed)
        std::fs::create_dir_all(pkg_dir.join("a")).unwrap();
        std::fs::write(
            pkg_dir.join("a").join("skill.md"),
            "---\nname: a-skill\ndescription: depth 1\n---\n\n```bash\necho a\n```\n",
        )
        .unwrap();

        // Depth 3: a/b/c/deep.md -> "mypkg a b c deep" (at cap, allowed)
        std::fs::create_dir_all(pkg_dir.join("a").join("b").join("c")).unwrap();
        std::fs::write(
            pkg_dir.join("a").join("b").join("c").join("deep.md"),
            "---\nname: deep\ndescription: depth 3\n---\n\n```bash\necho deep\n```\n",
        )
        .unwrap();

        // Depth 4: a/b/c/d/toodeep.md — should be skipped
        std::fs::create_dir_all(pkg_dir.join("a").join("b").join("c").join("d")).unwrap();
        std::fs::write(
            pkg_dir
                .join("a")
                .join("b")
                .join("c")
                .join("d")
                .join("toodeep.md"),
            "---\nname: toodeep\ndescription: too deep\n---\n\n```bash\necho deep\n```\n",
        )
        .unwrap();

        let ctx = AppContext::for_test_with_creft_home(
            dir.path().to_path_buf(),
            dir.path().to_path_buf(),
        );
        let skills = list_package_skills_in(&ctx, "mypkg", Scope::Global).unwrap();
        let names: Vec<&str> = skills.iter().map(|s| s.name.as_str()).collect();
        assert!(
            names.contains(&"mypkg a skill"),
            "expected mypkg a skill, got: {:?}",
            names
        );
        assert!(
            names.contains(&"mypkg a b c deep"),
            "expected mypkg a b c deep, got: {:?}",
            names
        );
        assert!(
            !names.iter().any(|n| n.contains("toodeep")),
            "toodeep should be skipped, got: {:?}",
            names
        );
    }

    // --- load_package_skill ---

    #[test]
    fn test_load_package_skill_not_found_package() {
        let dir = tempfile::TempDir::new().unwrap();
        let ctx = AppContext::for_test_with_creft_home(
            dir.path().to_path_buf(),
            dir.path().to_path_buf(),
        );

        let result = load_package_skill(&ctx, "nonexistent deploy");
        assert!(matches!(result, Err(CreftError::PackageNotFound(_))));
    }

    #[test]
    fn test_load_package_skill_not_found_file() {
        let dir = tempfile::TempDir::new().unwrap();

        let pkg_dir = dir.path().join("packages").join("mypkg");
        std::fs::create_dir_all(&pkg_dir).unwrap();

        let ctx = AppContext::for_test_with_creft_home(
            dir.path().to_path_buf(),
            dir.path().to_path_buf(),
        );
        let result = load_package_skill(&ctx, "mypkg missing-skill");
        assert!(matches!(result, Err(CreftError::PackageNotFound(_))));
    }

    #[test]
    fn test_load_package_skill_overrides_name() {
        let dir = tempfile::TempDir::new().unwrap();

        let pkg_dir = dir.path().join("packages").join("mypkg");
        std::fs::create_dir_all(&pkg_dir).unwrap();
        std::fs::write(
            pkg_dir.join("deploy.md"),
            "---\nname: frontmatter-name\ndescription: deploy something\n---\n\n```bash\necho deploy\n```\n",
        )
        .unwrap();

        let ctx = AppContext::for_test_with_creft_home(
            dir.path().to_path_buf(),
            dir.path().to_path_buf(),
        );
        let parsed = load_package_skill(&ctx, "mypkg deploy").unwrap();
        // The frontmatter name is overridden with the computed namespaced name
        assert_eq!(parsed.def.name, "mypkg deploy");
        assert_eq!(parsed.def.description, "deploy something");
        assert_eq!(parsed.blocks.len(), 1);
    }

    #[test]
    fn test_load_package_skill_nested() {
        let dir = tempfile::TempDir::new().unwrap();

        let pkg_dir = dir.path().join("packages").join("mypkg");
        std::fs::create_dir_all(pkg_dir.join("networking")).unwrap();
        std::fs::write(
            pkg_dir.join("networking").join("check-dns.md"),
            "---\nname: check-dns\ndescription: check DNS\n---\n\n```bash\ndig google.com\n```\n",
        )
        .unwrap();

        let ctx = AppContext::for_test_with_creft_home(
            dir.path().to_path_buf(),
            dir.path().to_path_buf(),
        );
        let parsed = load_package_skill(&ctx, "mypkg networking check-dns").unwrap();
        assert_eq!(parsed.def.name, "mypkg networking check-dns");
    }

    #[test]
    fn test_load_package_skill_too_few_tokens() {
        // Only a package name with no skill — should return PackageNotFound.
        // The function requires at least 2 tokens.
        let dir = tempfile::TempDir::new().unwrap();
        let ctx = AppContext::for_test_with_creft_home(
            dir.path().to_path_buf(),
            dir.path().to_path_buf(),
        );
        let result = load_package_skill(&ctx, "onlyone");
        assert!(matches!(result, Err(CreftError::PackageNotFound(_))));
    }

    // --- move_dir ---

    #[test]
    fn test_move_dir_same_filesystem() {
        let tmp = tempfile::TempDir::new().unwrap();
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");

        // Create src with a file and a subdirectory.
        std::fs::create_dir_all(src.join("sub")).unwrap();
        std::fs::write(src.join("file.txt"), "hello").unwrap();
        std::fs::write(src.join("sub").join("nested.txt"), "world").unwrap();

        move_dir(&src, &dst).unwrap();

        // Source should no longer exist; destination should have the contents.
        assert!(!src.exists(), "source directory should be gone after move");
        assert!(dst.exists(), "destination should exist");
        assert!(dst.join("file.txt").exists(), "file.txt should be at dest");
        assert!(
            dst.join("sub").join("nested.txt").exists(),
            "nested.txt should be at dest"
        );
        assert_eq!(
            std::fs::read_to_string(dst.join("file.txt")).unwrap(),
            "hello"
        );
        assert_eq!(
            std::fs::read_to_string(dst.join("sub").join("nested.txt")).unwrap(),
            "world"
        );
    }

    #[test]
    fn test_move_dir_empty_directory() {
        let tmp = tempfile::TempDir::new().unwrap();
        let src = tmp.path().join("empty-src");
        let dst = tmp.path().join("empty-dst");

        std::fs::create_dir_all(&src).unwrap();

        move_dir(&src, &dst).unwrap();

        assert!(!src.exists(), "source should be gone");
        assert!(dst.exists(), "destination should exist");
    }

    #[test]
    fn test_skill_file_path_rejects_traversal() {
        let dir = tempfile::TempDir::new().unwrap();
        let ctx = AppContext::for_test_with_creft_home(
            dir.path().to_path_buf(),
            dir.path().to_path_buf(),
        );

        // Create a package directory so the package-resolution branch is entered.
        let pkg_dir = dir.path().join("packages").join("mypkg");
        std::fs::create_dir_all(&pkg_dir).unwrap();

        // Attempt path traversal via the skill name token containing "..".
        let result = skill_file_path(&ctx, "mypkg ../something");
        assert!(
            result.is_err(),
            "expected error for traversal in skill name, got: {:?}",
            result
        );
    }

    // --- symlink skipping tests ---

    #[cfg(unix)]
    #[test]
    fn test_copy_dir_recursive_skips_symlinks() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::TempDir::new().unwrap();
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");
        let outside = tmp.path().join("outside.txt");

        // Create a regular file inside src.
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("regular.txt"), "regular content").unwrap();

        // Create a file outside src and symlink it from inside src.
        std::fs::write(&outside, "secret").unwrap();
        symlink(&outside, src.join("link.txt")).unwrap();

        copy_dir_recursive(&src, &dst).unwrap();

        // Regular file should be copied.
        assert!(
            dst.join("regular.txt").exists(),
            "regular file should be copied"
        );
        // Symlink should NOT be copied.
        assert!(
            !dst.join("link.txt").exists(),
            "symlinked file should not be copied"
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_copy_dir_recursive_skips_symlinked_dirs() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::TempDir::new().unwrap();
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");
        let outside_dir = tmp.path().join("outside_dir");

        // Create a regular subdirectory inside src.
        std::fs::create_dir_all(src.join("regular_sub")).unwrap();
        std::fs::write(src.join("regular_sub").join("file.txt"), "hello").unwrap();

        // Create a directory outside src and symlink it from inside src.
        std::fs::create_dir_all(&outside_dir).unwrap();
        std::fs::write(outside_dir.join("secret.txt"), "secret").unwrap();
        symlink(&outside_dir, src.join("linked_dir")).unwrap();

        copy_dir_recursive(&src, &dst).unwrap();

        // Regular subdirectory should be copied.
        assert!(
            dst.join("regular_sub").join("file.txt").exists(),
            "regular subdirectory contents should be copied"
        );
        // Symlinked directory should NOT be followed.
        assert!(
            !dst.join("linked_dir").exists(),
            "symlinked directory should not be copied"
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_collect_package_skills_skips_symlinks() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::TempDir::new().unwrap();

        let pkg_dir = dir.path().join("packages").join("mypkg");
        std::fs::create_dir_all(&pkg_dir).unwrap();

        // Regular skill file.
        std::fs::write(
            pkg_dir.join("real.md"),
            "---\nname: real\ndescription: real skill\n---\n\n```bash\necho real\n```\n",
        )
        .unwrap();

        // A skill file outside the package that we symlink into it.
        let outside = dir.path().join("outside.md");
        std::fs::write(
            &outside,
            "---\nname: outside\ndescription: outside skill\n---\n\n```bash\necho outside\n```\n",
        )
        .unwrap();
        symlink(&outside, pkg_dir.join("linked.md")).unwrap();

        let ctx = AppContext::for_test_with_creft_home(
            dir.path().to_path_buf(),
            dir.path().to_path_buf(),
        );
        let skills = list_package_skills_in(&ctx, "mypkg", Scope::Global).unwrap();
        let names: Vec<&str> = skills.iter().map(|s| s.name.as_str()).collect();

        assert_eq!(
            skills.len(),
            1,
            "only the real skill should be collected, got: {:?}",
            names
        );
        assert_eq!(names[0], "mypkg real");
    }

    // --- packages_dir_for ---

    #[test]
    fn test_packages_dir_creft_home_both_scopes_resolve_to_same_path() {
        // Verify that packages_dir_for (via AppContext) resolves correctly under CREFT_HOME.
        let dir = tempfile::TempDir::new().unwrap();
        let ctx = AppContext::for_test_with_creft_home(
            dir.path().to_path_buf(),
            dir.path().to_path_buf(),
        );

        // With CREFT_HOME set, both scopes resolve to the same path.
        let local_pkgs = ctx.packages_dir_for(Scope::Local).unwrap();
        let global_pkgs = ctx.packages_dir_for(Scope::Global).unwrap();
        assert_eq!(local_pkgs, global_pkgs);
        assert_eq!(local_pkgs, dir.path().join("packages"));
    }

    #[test]
    fn test_packages_dir_for_respects_scope_under_creft_home() {
        let dir = tempfile::TempDir::new().unwrap();
        let ctx = AppContext::for_test_with_creft_home(
            dir.path().to_path_buf(),
            dir.path().to_path_buf(),
        );

        // With CREFT_HOME set, both scopes resolve to the same packages dir.
        let local = ctx.packages_dir_for(Scope::Local).unwrap();
        let global = ctx.packages_dir_for(Scope::Global).unwrap();
        assert_eq!(local, global);
    }
}
