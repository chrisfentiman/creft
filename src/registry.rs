use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::catalog::{self, CatalogEntry};
use crate::error::CreftError;
use crate::frontmatter;
use crate::markdown;
use crate::model::{AppContext, CommandDef, ParsedCommand, Scope};
use crate::store;

/// Manifest for an installed skill package.
#[derive(Debug, Clone)]
pub struct PackageManifest {
    pub name: String,
    pub version: String,
    #[allow(dead_code)] // parsed from manifest YAML
    pub description: String,
    #[allow(dead_code)] // parsed from manifest YAML
    pub author: Option<String>,
    #[allow(dead_code)] // parsed from manifest YAML
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

/// Read a `PackageManifest` from an installed package directory.
///
/// Reads `.creft/catalog.json`. Returns `None` if no catalog exists.
pub(crate) fn read_manifest_from(path: &Path) -> Option<Result<PackageManifest, String>> {
    let catalog_path = path.join(".creft").join("catalog.json");
    if catalog_path.exists() {
        return Some(
            std::fs::read_to_string(&catalog_path)
                .map_err(|e| e.to_string())
                .and_then(|content| {
                    let label = catalog_path.display().to_string();
                    catalog::parse_catalog(&content, &label)
                        .map_err(|e| e.to_string())
                        .and_then(|cat| {
                            cat.plugins
                                .into_iter()
                                .next()
                                .map(|entry| PackageManifest {
                                    name: entry.name,
                                    version: entry.version.unwrap_or_else(|| "0.0.0".into()),
                                    description: entry.description,
                                    author: None,
                                    license: None,
                                })
                                .ok_or_else(|| "catalog has no plugin entries".to_string())
                        })
                }),
        );
    }

    None
}

/// List all installed packages in the given scope.
///
/// Reads `ctx.packages_dir_for(scope)` and parses `.creft/catalog.json` for each
/// subdirectory. Entries that fail to parse are skipped with a warning to stderr.
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
        match read_manifest_from(&path) {
            Some(Ok(manifest)) => packages.push(InstalledPackage { manifest, path }),
            Some(Err(e)) => eprintln!(
                "warning: skipping {}: invalid manifest: {}",
                path.display(),
                e
            ),
            None => {
                // No manifest found — skip silently (directory may not be a package).
            }
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
/// Skips dotfiles and dot-directories.
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

        if file_name.eq_ignore_ascii_case("README.md") {
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
/// When the file is at the package root and its stem matches the package name,
/// the stem is omitted to avoid redundant names (e.g. `fetch/fetch.md` → `"fetch"`
/// instead of `"fetch fetch"`).
///
/// Examples:
/// - `"fetch"`, `"fetch.md"` -> `"fetch"` (dedup: root file matches package name)
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
        let at_root = rel.parent().is_none_or(|p| p == Path::new(""));
        if at_root && stem == pkg_name {
            // Root-level file matching package name — skip the stem to avoid
            // "fetch fetch" when the package and file share the same name.
        } else {
            parts.push(stem.to_string());
        }
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

/// The creft repo shorthand resolves to this URL.
const CREFT_REPO: &str = "https://github.com/chrisfentiman/creft";

/// Install a plugin from a git URL into the global plugin cache.
///
/// 1. Clone the repo to a temp directory with `git clone --depth 1`.
/// 2. Read `.creft/catalog.json` from the cloned repo root.
/// 3. For multi-plugin repos, select the entry named by `plugin_filter`,
///    or return an error listing available plugins when no filter is given.
/// 4. Move or copy the plugin to `ctx.plugins_dir()/<name>/`.
///
/// Catalog entries with a `Path` source are installed from within the cloned
/// repo via `install_from_path`. All other entry types are treated as the
/// entire cloned repo.
pub fn plugin_install(
    ctx: &AppContext,
    url: &str,
    plugin_filter: Option<&str>,
) -> Result<InstalledPackage, CreftError> {
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

    let catalog_path = tmp_path.join(".creft").join("catalog.json");
    if !catalog_path.exists() {
        return Err(CreftError::ManifestNotFound);
    }

    let catalog_content = std::fs::read_to_string(&catalog_path)?;
    let cat = catalog::parse_catalog(&catalog_content, url)?;

    if cat.plugins.is_empty() {
        return Err(CreftError::InvalidManifest(
            "catalog has no plugin entries".into(),
        ));
    }

    let entry: &CatalogEntry = if cat.plugins.len() > 1 {
        // Multi-plugin repo: a filter is required.
        match plugin_filter {
            Some(name) => cat.plugins.iter().find(|p| p.name == name).ok_or_else(|| {
                let available: Vec<&str> = cat.plugins.iter().map(|p| p.name.as_str()).collect();
                CreftError::PluginNotInCatalog {
                    catalog: url.to_string(),
                    plugin: name.to_string(),
                    available: available.join(", "),
                }
            })?,
            None => {
                let names: Vec<&str> = cat.plugins.iter().map(|p| p.name.as_str()).collect();
                return Err(CreftError::InvalidManifest(format!(
                    "repository contains {} plugins: {}. Use 'creft plugin install owner/<name>' to install a specific one",
                    cat.plugins.len(),
                    names.join(", ")
                )));
            }
        }
    } else {
        // Single-plugin repo: validate the filter if provided.
        let single = &cat.plugins[0];
        if let Some(name) = plugin_filter
            && single.name != name
        {
            return Err(CreftError::PluginNotInCatalog {
                catalog: url.to_string(),
                plugin: name.to_string(),
                available: single.name.clone(),
            });
        }
        single
    };

    // For path-based entries, the plugin files live inside the cloned repo.
    if let catalog::PluginSource::Path(ref rel_path) = entry.source {
        let source_path = tmp_path.join(rel_path.trim_start_matches("./"));
        return install_from_path(ctx, &source_path, entry);
    }

    // For non-path entries (the whole repo is the plugin), move the clone.
    let manifest = PackageManifest {
        name: entry.name.clone(),
        version: entry.version.clone().unwrap_or_else(|| "0.0.0".into()),
        description: entry.description.clone(),
        author: None,
        license: None,
    };
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

/// Install a plugin from a local directory path (path-based catalog entries).
///
/// Copies the directory into the global plugin cache and writes a synthetic
/// `.creft/catalog.json` so `list_packages_in` and `doctor` can read metadata.
pub fn install_from_path(
    ctx: &AppContext,
    source_path: &Path,
    entry: &CatalogEntry,
) -> Result<InstalledPackage, CreftError> {
    let manifest = PackageManifest {
        name: entry.name.clone(),
        version: entry.version.clone().unwrap_or_else(|| "0.0.0".into()),
        description: entry.description.clone(),
        author: None,
        license: None,
    };
    validate_manifest_name(&manifest.name)?;

    let plugins = ctx.plugins_dir()?;
    let dest = plugins.join(&manifest.name);
    if dest.exists() {
        return Err(CreftError::PackageAlreadyInstalled(manifest.name.clone()));
    }
    if !plugins.exists() {
        std::fs::create_dir_all(&plugins)?;
    }

    copy_dir_recursive(source_path, &dest)?;

    // Write a synthetic catalog only when the copy did not bring one over.
    // This happens when the plugin is a subdirectory of the catalog repo
    // without its own .creft/catalog.json. When source is "." (the whole repo),
    // the real catalog was already copied and must not be overwritten — the
    // installed directory may have a live .git, and overwriting a tracked file
    // would cause `git pull` to abort with "local changes would be overwritten".
    let catalog_dir = dest.join(".creft");
    let catalog_file = catalog_dir.join("catalog.json");
    if !catalog_file.exists() {
        let synthetic_catalog = serde_json::json!({
            "name": entry.name,
            "description": entry.description,
            "plugins": [{
                "name": entry.name,
                "source": ".",
                "description": entry.description,
                "version": entry.version.clone().unwrap_or_else(|| "0.0.0".into()),
                "tags": entry.tags,
            }]
        });
        std::fs::create_dir_all(&catalog_dir)?;
        std::fs::write(
            &catalog_file,
            serde_json::to_string_pretty(&synthetic_catalog)
                .map_err(|e| CreftError::Serialization(e.to_string()))?,
        )?;
    }

    Ok(InstalledPackage {
        manifest,
        path: dest,
    })
}

/// Install a plugin by shorthand name.
///
/// `creft/<plugin>` resolves to the official creft repo. All other
/// `owner/repo` patterns resolve to `https://github.com/<owner>/<repo>`.
/// The plugin name is always derived from the path segment after `/`.
/// Bare names (no `/`) return an error.
pub fn install_by_name(
    ctx: &AppContext,
    qualified_name: &str,
) -> Result<InstalledPackage, CreftError> {
    let Some(slash_pos) = qualified_name.find('/') else {
        return Err(CreftError::InvalidManifest(format!(
            "'{qualified_name}' is not a valid plugin source — use owner/repo or a full git URL"
        )));
    };

    let owner = &qualified_name[..slash_pos];
    let plugin = &qualified_name[slash_pos + 1..];

    if owner == "creft" {
        // creft shorthand: clone the official repo and install the named plugin.
        plugin_install(ctx, CREFT_REPO, Some(plugin))
    } else {
        // Treat as GitHub owner/repo shorthand. The plugin name is the repo
        // name (the part after `/`), which is always derivable from the source.
        let url = format!("https://github.com/{owner}/{plugin}");
        plugin_install(ctx, &url, Some(plugin))
    }
}

/// Update an installed plugin by running `git pull --ff-only` in its directory.
///
/// After pulling, re-reads the manifest from `.creft/catalog.json` to return updated metadata.
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

    let manifest = read_manifest_from(&plugin_path)
        .ok_or(CreftError::ManifestNotFound)?
        .map_err(CreftError::InvalidManifest)?;

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
///
/// Skill names include the plugin name as the first token, consistent with
/// how `creft list` groups namespaced commands. Use
/// `list_plugin_skills_unprefixed` when resolving skills by short name.
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

/// List skill definitions in an installed plugin without the plugin name prefix.
///
/// `hello.md` in plugin `my-tools` produces skill name `"hello"` rather than
/// `"my-tools hello"`. This form is used when adding plugin skills to the
/// global skill list — the plugin name is tracked in `SkillSource::Plugin` instead.
pub fn list_plugin_skills_unprefixed(
    ctx: &AppContext,
    plugin_name: &str,
) -> Result<Vec<CommandDef>, CreftError> {
    let plugin_dir = ctx.plugins_dir()?.join(plugin_name);
    if !plugin_dir.exists() {
        return Err(CreftError::PackageNotFound(plugin_name.to_string()));
    }

    let mut skills = Vec::new();
    collect_plugin_skills_unprefixed(&plugin_dir, &plugin_dir, 0, &mut skills)?;
    skills.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(skills)
}

/// Recursively collect `.md` skills from a plugin directory, naming them without
/// the plugin name prefix.
///
/// `hello.md` at the plugin root becomes `"hello"`, not `"plugin-name hello"`.
fn collect_plugin_skills_unprefixed(
    plugin_root: &Path,
    dir: &Path,
    depth: usize,
    skills: &mut Vec<CommandDef>,
) -> Result<(), CreftError> {
    let entries = std::fs::read_dir(dir)?;
    for entry in entries {
        let entry = entry?;
        let file_type = entry.file_type()?;

        if file_type.is_symlink() {
            continue;
        }

        let path = entry.path();
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        if file_name.starts_with('.') {
            continue;
        }

        if file_type.is_dir() {
            if depth < 3 {
                collect_plugin_skills_unprefixed(plugin_root, &path, depth + 1, skills)?;
            }
            continue;
        }

        if path.extension().is_none_or(|e| e != "md") {
            continue;
        }

        if file_name.eq_ignore_ascii_case("README.md") {
            continue;
        }

        let rel = path
            .strip_prefix(plugin_root)
            .expect("path must be under plugin_root");

        // Build skill name from path components, without the plugin name.
        let mut parts = Vec::new();
        if let Some(parent) = rel.parent() {
            for component in parent.components() {
                parts.push(component.as_os_str().to_string_lossy().into_owned());
            }
        }
        if let Some(stem) = rel.file_stem().and_then(|s| s.to_str()) {
            parts.push(stem.to_string());
        }
        let skill_name = parts.join(" ");

        match std::fs::read_to_string(&path) {
            Ok(content) => match crate::frontmatter::parse(&content) {
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

/// Activation state for all plugins in a single scope.
///
/// Stored at `.creft/plugins/settings.json` (local scope) or
/// `~/.creft/plugins/settings.json` (global scope).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PluginSettings {
    /// Activated plugin commands.
    ///
    /// Keys are plugin names. Values describe which commands are active.
    /// An `All(true)` value means every command from the plugin is active.
    /// A `Commands(list)` value means only the listed command names are active.
    #[serde(default)]
    pub activated: std::collections::BTreeMap<String, ActivationEntry>,
}

/// Activation state for a single plugin within a scope.
///
/// Deserialization: `true` → `All(true)`, `["cmd1", "cmd2"]` → `Commands(vec)`.
/// `false` is rejected by `PluginSettings::validate`. An empty command list
/// is normalized to `All(true)` by `validate` — key presence means active.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ActivationEntry {
    /// All commands from this plugin are active.
    All(bool),
    /// Only specific named commands are active.
    Commands(Vec<String>),
}

impl PluginSettings {
    /// Validate and normalize settings after deserialization.
    ///
    /// - Rejects `All(false)`: to deactivate a plugin, remove its key.
    /// - Normalizes `Commands([])` to `All(true)`: empty list is equivalent to all.
    pub fn validate(&mut self) -> Result<(), CreftError> {
        let mut to_normalize = Vec::new();
        for (name, entry) in &self.activated {
            match entry {
                ActivationEntry::All(false) => {
                    return Err(CreftError::InvalidManifest(format!(
                        "plugin '{}' has activation value 'false'; \
                         remove the key to deactivate instead",
                        name
                    )));
                }
                ActivationEntry::Commands(cmds) if cmds.is_empty() => {
                    to_normalize.push(name.clone());
                }
                _ => {}
            }
        }
        for name in to_normalize {
            self.activated.insert(name, ActivationEntry::All(true));
        }
        Ok(())
    }
}

/// Load activation settings for a scope from `settings.json`.
///
/// Returns a default (empty) `PluginSettings` when the file does not exist.
/// Calls `validate()` after deserialization to reject invalid states.
pub fn load_settings(ctx: &AppContext, scope: Scope) -> Result<PluginSettings, CreftError> {
    let path = ctx.plugin_settings_path(scope)?;
    if !path.exists() {
        return Ok(PluginSettings::default());
    }
    let content = std::fs::read_to_string(&path)?;
    let mut settings: PluginSettings = serde_json::from_str(&content)
        .map_err(|e| CreftError::InvalidManifest(format!("settings.json parse error: {e}")))?;
    settings.validate()?;
    Ok(settings)
}

/// Save activation settings for a scope to `settings.json`.
///
/// Creates the `.creft/plugins/` directory if it does not exist.
pub fn save_settings(
    ctx: &AppContext,
    scope: Scope,
    settings: &PluginSettings,
) -> Result<(), CreftError> {
    let path = ctx.plugin_settings_path(scope)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let content = serde_json::to_string_pretty(settings)
        .map_err(|e| CreftError::Serialization(e.to_string()))?;
    std::fs::write(&path, content)?;
    Ok(())
}

/// Activate all commands for an already-validated installed plugin.
///
/// Writes `All(true)` into the plugin's activation entry and persists settings.
/// Callers must verify the plugin directory exists before calling this.
fn activate_plugin_all(
    ctx: &AppContext,
    plugin_name: &str,
    scope: Scope,
) -> Result<(), CreftError> {
    let mut settings = load_settings(ctx, scope)?;
    settings
        .activated
        .insert(plugin_name.to_string(), ActivationEntry::All(true));
    save_settings(ctx, scope, &settings)
}

/// Activate a command or all commands from an installed plugin.
///
/// `target` is either `plugin` (activate all), `plugin/cmd` (one command),
/// or `owner/plugin` (qualified install name — the owner prefix is stripped).
/// The qualified-name form lets callers use `creft plugin activate creft/ask`
/// interchangeably with `creft plugin activate ask`.
///
/// Validates the plugin is installed and the command exists before writing.
/// Writes activation state to the `settings.json` for the given scope.
pub fn activate(ctx: &AppContext, target: &str, scope: Scope) -> Result<(), CreftError> {
    let (plugin_name, cmd_name) = parse_activation_target(target);

    let plugins_dir = ctx.plugins_dir()?;
    let plugin_dir = plugins_dir.join(plugin_name);
    if !plugin_dir.is_dir() {
        // The first segment isn't an installed plugin. If the input looks like
        // `owner/plugin` (i.e. cmd_name is Some and that name IS installed),
        // treat it as a qualified install name and activate the second segment.
        if let Some(qualified_plugin) = cmd_name {
            let alt_dir = plugins_dir.join(qualified_plugin);
            if alt_dir.is_dir() {
                return activate_plugin_all(ctx, qualified_plugin, scope);
            }
        }
        return Err(CreftError::PackageNotFound(plugin_name.to_string()));
    }

    if let Some(cmd) = cmd_name {
        // Validate the specific command exists.
        plugin_skill_file_path(ctx, plugin_name, cmd)?;
    }

    let mut settings = load_settings(ctx, scope)?;

    if let Some(cmd) = cmd_name {
        match settings.activated.get_mut(plugin_name) {
            Some(ActivationEntry::All(true)) => {
                // Already all-active; no change needed.
            }
            Some(ActivationEntry::Commands(cmds)) => {
                if !cmds.contains(&cmd.to_string()) {
                    cmds.push(cmd.to_string());
                }
            }
            _ => {
                settings.activated.insert(
                    plugin_name.to_string(),
                    ActivationEntry::Commands(vec![cmd.to_string()]),
                );
            }
        }
    } else {
        settings
            .activated
            .insert(plugin_name.to_string(), ActivationEntry::All(true));
    }

    save_settings(ctx, scope, &settings)?;
    Ok(())
}

/// Deactivate a command or all commands from a plugin.
///
/// When `global_only` is false, removes the entry from every scope that contains
/// it (local first, then global). This matches the user's intent: "make this
/// stop appearing" should clear all scopes, not leave a stale activation elsewhere.
///
/// When `global_only` is true, only the global scope is checked and modified.
///
/// Returns `ActivationNotFound` when the target exists in no checked scope.
pub fn deactivate(ctx: &AppContext, target: &str, global_only: bool) -> Result<(), CreftError> {
    let (plugin_name, cmd_name) = parse_activation_target(target);

    let scopes_to_check: Vec<Scope> = if global_only {
        vec![Scope::Global]
    } else {
        // Always check global; check local only when a local root exists.
        let mut scopes = Vec::new();
        if ctx.find_local_root().is_some() {
            scopes.push(Scope::Local);
        }
        scopes.push(Scope::Global);
        scopes
    };

    let mut found_in_any = false;

    for scope in scopes_to_check {
        let mut settings = load_settings(ctx, scope)?;
        let changed = remove_from_settings(&mut settings, plugin_name, cmd_name);
        if changed {
            found_in_any = true;
            save_settings(ctx, scope, &settings)?;
        }
    }

    if !found_in_any {
        return Err(CreftError::ActivationNotFound {
            plugin: plugin_name.to_string(),
            cmd: cmd_name.unwrap_or("(all)").to_string(),
        });
    }

    Ok(())
}

/// Remove a plugin or specific command from settings. Returns true if anything changed.
fn remove_from_settings(
    settings: &mut PluginSettings,
    plugin_name: &str,
    cmd_name: Option<&str>,
) -> bool {
    match cmd_name {
        None => {
            // Deactivate the whole plugin.
            settings.activated.remove(plugin_name).is_some()
        }
        Some(cmd) => {
            match settings.activated.get_mut(plugin_name) {
                Some(ActivationEntry::All(true)) => {
                    // Can't selectively remove from "all" — remove the whole entry.
                    settings.activated.remove(plugin_name);
                    true
                }
                Some(ActivationEntry::Commands(cmds)) => {
                    let before = cmds.len();
                    cmds.retain(|c| c != cmd);
                    let removed = cmds.len() < before;
                    // Clean up empty command list.
                    if cmds.is_empty() {
                        settings.activated.remove(plugin_name);
                    }
                    removed
                }
                _ => false,
            }
        }
    }
}

/// Parse a `plugin` or `plugin/cmd` activation target string.
///
/// Returns `(plugin_name, None)` for whole-plugin targets and
/// `(plugin_name, Some(cmd_name))` for specific-command targets.
fn parse_activation_target(target: &str) -> (&str, Option<&str>) {
    if let Some(idx) = target.find('/') {
        let plugin = &target[..idx];
        let cmd = &target[idx + 1..];
        (plugin, Some(cmd))
    } else {
        (target, None)
    }
}

/// Resolve the filesystem path for a skill within an installed plugin.
///
/// Looks in `plugins_dir()/<plugin_name>/`. When `skill_name` equals
/// `plugin_name` (the dedup case: `fetch/fetch.md` → name `"fetch"`),
/// looks for `<plugin_name>.md` at the plugin root. Otherwise, treats
/// whitespace-separated tokens as path components with the last getting
/// a `.md` suffix.
pub fn plugin_skill_file_path(
    ctx: &AppContext,
    plugin_name: &str,
    skill_name: &str,
) -> Result<PathBuf, CreftError> {
    let plugin_dir = ctx.plugins_dir()?.join(plugin_name);
    if !plugin_dir.is_dir() {
        return Err(CreftError::PackageNotFound(plugin_name.to_string()));
    }

    if skill_name == plugin_name {
        // Dedup case: the skill file is at the plugin root with a matching name.
        let path = plugin_dir.join(format!("{}.md", plugin_name));
        if path.exists() {
            return Ok(path);
        }
        return Err(CreftError::PackageNotFound(skill_name.to_string()));
    }

    let tokens: Vec<&str> = skill_name.split_whitespace().collect();
    for token in &tokens {
        store::validate_path_token(token)?;
    }
    let mut file_path = plugin_dir;
    for (i, token) in tokens.iter().enumerate() {
        if i == tokens.len() - 1 {
            file_path = file_path.join(format!("{}.md", token));
        } else {
            file_path = file_path.join(token);
        }
    }
    if file_path.exists() {
        return Ok(file_path);
    }
    Err(CreftError::PackageNotFound(skill_name.to_string()))
}

/// A skill match returned by `plugin_search`.
#[derive(Debug, Clone)]
pub struct PluginSkillMatch {
    /// The plugin that contains this skill.
    pub plugin_name: String,
    /// The skill definition (name set to the full skill name).
    pub def: crate::model::CommandDef,
}

/// Search for skills across all installed plugins.
///
/// Returns every skill from every installed plugin whose name, description, or
/// tags contain at least one of the `query` terms (case-insensitive substring).
/// When `query` is empty, all skills from all plugins are returned.
///
/// Skills are returned sorted by plugin name, then by skill name within each plugin.
pub fn plugin_search(
    ctx: &AppContext,
    query: &[String],
) -> Result<Vec<PluginSkillMatch>, CreftError> {
    let plugins_dir = ctx.plugins_dir()?;
    if !plugins_dir.exists() {
        return Ok(Vec::new());
    }

    // Normalize query terms once — case-insensitive substring match.
    let terms: Vec<String> = query.iter().map(|t| t.to_lowercase()).collect();

    let mut matches: Vec<PluginSkillMatch> = Vec::new();

    let mut plugin_names: Vec<String> = std::fs::read_dir(&plugins_dir)?
        .filter_map(|e| {
            let e = e.ok()?;
            if e.file_type().ok()?.is_dir() {
                e.file_name().into_string().ok()
            } else {
                None
            }
        })
        .collect();
    plugin_names.sort();

    for plugin_name in plugin_names {
        let skills = list_plugin_skills_in(ctx, &plugin_name)?;
        for def in skills {
            if skill_matches_query(&def, &terms) {
                matches.push(PluginSkillMatch {
                    plugin_name: plugin_name.clone(),
                    def,
                });
            }
        }
    }

    Ok(matches)
}

/// Returns `true` if `def` matches all of the given query `terms`.
///
/// An empty `terms` slice matches everything. Each term must appear as a
/// case-insensitive substring in at least one of: skill name, description,
/// or any tag. All terms must match (AND semantics).
fn skill_matches_query(def: &crate::model::CommandDef, terms: &[String]) -> bool {
    if terms.is_empty() {
        return true;
    }

    let name_lower = def.name.to_lowercase();
    let desc_lower = def.description.to_lowercase();
    let tags_lower: Vec<String> = def.tags.iter().map(|t| t.to_lowercase()).collect();

    terms.iter().all(|term| {
        name_lower.contains(term.as_str())
            || desc_lower.contains(term.as_str())
            || tags_lower.iter().any(|tag| tag.contains(term.as_str()))
    })
}

/// Load and parse a skill from an installed plugin.
pub fn load_plugin_skill(
    ctx: &AppContext,
    plugin_name: &str,
    skill_name: &str,
) -> Result<ParsedCommand, CreftError> {
    let file_path = plugin_skill_file_path(ctx, plugin_name, skill_name)?;
    let content = std::fs::read_to_string(&file_path)?;
    let (mut def, body) = crate::frontmatter::parse(&content)?;
    let (docs, blocks) = crate::markdown::extract_blocks(&body);
    def.name = skill_name.to_string();
    Ok(ParsedCommand { def, docs, blocks })
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
        let manifest: PackageManifest = crate::yaml::from_str(yaml).unwrap();
        assert_eq!(manifest.name, "k8s-tools");
        assert_eq!(manifest.version, "0.1.0");
        assert_eq!(manifest.description, "Kubernetes skills");
        assert_eq!(manifest.author, Some("someone".into()));
        assert_eq!(manifest.license, Some("MIT".into()));
    }

    #[test]
    fn test_manifest_optional_fields_absent() {
        let yaml = "name: my-pkg\nversion: 1.0.0\ndescription: A package\n";
        let manifest: PackageManifest = crate::yaml::from_str(yaml).unwrap();
        assert_eq!(manifest.name, "my-pkg");
        assert!(manifest.author.is_none());
        assert!(manifest.license.is_none());
    }

    #[test]
    fn test_manifest_missing_required_field_name() {
        let yaml = "version: 1.0.0\ndescription: A package\n";
        let result: Result<PackageManifest, crate::yaml::YamlError> = crate::yaml::from_str(yaml);
        assert!(result.is_err());
    }

    #[test]
    fn test_manifest_missing_required_field_version() {
        let yaml = "name: my-pkg\ndescription: A package\n";
        let result: Result<PackageManifest, crate::yaml::YamlError> = crate::yaml::from_str(yaml);
        assert!(result.is_err());
    }

    #[test]
    fn test_manifest_missing_required_field_description() {
        let yaml = "name: my-pkg\nversion: 1.0.0\n";
        let result: Result<PackageManifest, crate::yaml::YamlError> = crate::yaml::from_str(yaml);
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

    /// Top-level builtins are reserved and cannot be used as package names.
    #[rstest]
    #[case::add("add")]
    #[case::list("list")]
    #[case::show("show")]
    #[case::remove("remove")]
    #[case::plugin("plugin")]
    #[case::settings("settings")]
    #[case::up("up")]
    #[case::help("help")]
    #[case::version("version")]
    #[case::init("init")]
    #[case::doctor("doctor")]
    #[case::completions("completions")]
    fn reserved_builtins_are_rejected(#[case] name: &str) {
        let err = validate_manifest_name(name).unwrap_err();
        assert!(
            matches!(err, CreftError::InvalidManifest(_)),
            "{name} should be rejected as reserved"
        );
    }

    /// Former namespace names (`cmd`, `plugins`) are no longer reserved and can be
    /// used as package names.
    #[rstest]
    #[case::cmd("cmd")]
    #[case::plugins("plugins")]
    #[case::install("install")]
    #[case::update("update")]
    #[case::uninstall("uninstall")]
    fn former_namespace_names_are_valid(#[case] name: &str) {
        assert!(
            validate_manifest_name(name).is_ok(),
            "{name} should be valid as a package name"
        );
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

    // Stage 3: dedup — root-level file matching package name omits the stem.
    #[rstest]
    #[case::root_match("fetch", "fetch.md", "fetch")]
    #[case::root_mismatch("foo", "bar.md", "foo bar")]
    #[case::subdir_match("fetch", "sub/fetch.md", "fetch sub fetch")]
    #[case::subdir_different("k8s-tools", "sub/deploy.md", "k8s-tools sub deploy")]
    fn skill_name_from_path_dedup(
        #[case] pkg_name: &str,
        #[case] rel_str: &str,
        #[case] expected: &str,
    ) {
        let rel = Path::new(rel_str);
        assert_eq!(skill_name_from_path(pkg_name, rel), expected);
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

        let pkg_dir = dir.path().join("packages").join("my-pkg");
        std::fs::create_dir_all(pkg_dir.join(".creft")).unwrap();
        std::fs::write(
            pkg_dir.join(".creft").join("catalog.json"),
            r#"{"name":"my-pkg","description":"A test package","plugins":[{"name":"my-pkg","source":".","description":"A test package","version":"1.0.0","tags":[]}]}"#,
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
        std::fs::create_dir_all(pkg1.join(".creft")).unwrap();
        std::fs::write(
            pkg1.join(".creft").join("catalog.json"),
            r#"{"name":"good-pkg","description":"Good","plugins":[{"name":"good-pkg","source":".","description":"Good","version":"1.0.0","tags":[]}]}"#,
        )
        .unwrap();

        // Invalid package — malformed catalog.json
        let pkg2 = dir.path().join("packages").join("bad-pkg");
        std::fs::create_dir_all(pkg2.join(".creft")).unwrap();
        std::fs::write(pkg2.join(".creft").join("catalog.json"), "not valid json [").unwrap();

        let ctx = AppContext::for_test_with_creft_home(
            dir.path().to_path_buf(),
            dir.path().to_path_buf(),
        );
        let pkgs = list_packages_in(&ctx, Scope::Global).unwrap();
        assert_eq!(pkgs.len(), 1);
        assert_eq!(pkgs[0].manifest.name, "good-pkg");
    }

    // --- read_manifest_from ---

    #[test]
    fn read_manifest_from_reads_catalog_json() {
        let dir = tempfile::TempDir::new().unwrap();
        let catalog_dir = dir.path().join(".creft");
        std::fs::create_dir_all(&catalog_dir).unwrap();
        std::fs::write(
            catalog_dir.join("catalog.json"),
            r#"{"name":"my-pkg","description":"desc","plugins":[{"name":"my-pkg","source":".","description":"From catalog","version":"2.0.0","tags":[]}]}"#,
        )
        .unwrap();

        let manifest = read_manifest_from(dir.path()).unwrap().unwrap();
        assert_eq!(manifest.version, "2.0.0");
        assert_eq!(manifest.description, "From catalog");
    }

    #[test]
    fn read_manifest_from_returns_none_when_no_manifest() {
        let dir = tempfile::TempDir::new().unwrap();
        assert!(read_manifest_from(dir.path()).is_none());
    }

    #[test]
    fn read_manifest_from_ignores_directory_without_catalog() {
        let dir = tempfile::TempDir::new().unwrap();
        // Directory exists but contains no .creft/catalog.json.
        std::fs::create_dir_all(dir.path().join("some-files")).unwrap();
        assert!(read_manifest_from(dir.path()).is_none());
    }

    #[test]
    fn list_packages_reads_catalog_json() {
        let dir = tempfile::TempDir::new().unwrap();
        let pkg_dir = dir.path().join("packages").join("catalog-pkg");
        std::fs::create_dir_all(pkg_dir.join(".creft")).unwrap();
        std::fs::write(
            pkg_dir.join(".creft").join("catalog.json"),
            r#"{"name":"catalog-pkg","description":"","plugins":[{"name":"catalog-pkg","source":".","description":"A plugin","version":"3.0.0","tags":[]}]}"#,
        )
        .unwrap();

        let ctx = AppContext::for_test_with_creft_home(
            dir.path().to_path_buf(),
            dir.path().to_path_buf(),
        );
        let pkgs = list_packages_in(&ctx, Scope::Global).unwrap();
        assert_eq!(pkgs.len(), 1);
        assert_eq!(pkgs[0].manifest.name, "catalog-pkg");
        assert_eq!(pkgs[0].manifest.version, "3.0.0");
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
    fn test_list_package_skills_excludes_non_md_files() {
        let dir = tempfile::TempDir::new().unwrap();

        let pkg_dir = dir.path().join("packages").join("mypkg");
        std::fs::create_dir_all(pkg_dir.join(".creft")).unwrap();
        std::fs::write(
            pkg_dir.join(".creft").join("catalog.json"),
            r#"{"name":"mypkg","description":"","plugins":[{"name":"mypkg","source":".","description":"A package","version":"1.0.0","tags":[]}]}"#,
        )
        .unwrap();

        // A non-.md file — should be excluded by extension filter
        std::fs::write(pkg_dir.join("notes.txt"), "just some notes").unwrap();

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
    fn test_list_package_skills_excludes_readme() {
        let dir = tempfile::TempDir::new().unwrap();

        let pkg_dir = dir.path().join("packages").join("mypkg");
        std::fs::create_dir_all(&pkg_dir).unwrap();

        // Real skill
        std::fs::write(
            pkg_dir.join("skill.md"),
            "---\nname: skill\ndescription: a skill\n---\n\n```bash\necho ok\n```\n",
        )
        .unwrap();

        // README.md must not appear as a skill.
        std::fs::write(
            pkg_dir.join("README.md"),
            "# mypkg\n\nInstallation instructions.\n",
        )
        .unwrap();

        let ctx = AppContext::for_test_with_creft_home(
            dir.path().to_path_buf(),
            dir.path().to_path_buf(),
        );
        let skills = list_package_skills_in(&ctx, "mypkg", Scope::Global).unwrap();
        let names: Vec<&str> = skills.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(
            skills.len(),
            1,
            "README.md must not appear as a skill, got: {:?}",
            names
        );
        assert_eq!(names[0], "mypkg skill");
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

    #[test]
    fn test_list_plugin_skills_excludes_readme() {
        let dir = tempfile::TempDir::new().unwrap();
        let ctx = AppContext::for_test_with_creft_home(
            dir.path().to_path_buf(),
            dir.path().to_path_buf(),
        );

        let plugin_dir = dir.path().join("plugins").join("myplugin");
        std::fs::create_dir_all(&plugin_dir).unwrap();

        std::fs::write(
            plugin_dir.join("myplugin.md"),
            "---\nname: myplugin\ndescription: a plugin skill\n---\n\n```bash\necho ok\n```\n",
        )
        .unwrap();

        // README.md must not appear as a plugin skill.
        std::fs::write(
            plugin_dir.join("README.md"),
            "# myplugin\n\nUsage instructions.\n",
        )
        .unwrap();

        let skills = list_plugin_skills_in(&ctx, "myplugin").unwrap();
        let names: Vec<&str> = skills.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(
            skills.len(),
            1,
            "README.md must not appear as a plugin skill, got: {:?}",
            names
        );
        assert_eq!(names[0], "myplugin");
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

    // --- activate: owner/plugin qualified name fallback ---

    fn make_plugin_dir(creft_home: &std::path::Path, plugin_name: &str) {
        let plugins = creft_home.join("plugins");
        std::fs::create_dir_all(plugins.join(plugin_name)).unwrap();
    }

    fn make_plugin_skill(creft_home: &std::path::Path, plugin_name: &str, skill_file: &str) {
        let plugin_dir = creft_home.join("plugins").join(plugin_name);
        std::fs::create_dir_all(&plugin_dir).unwrap();
        std::fs::write(plugin_dir.join(skill_file), b"---\nname: test\n---\n").unwrap();
    }

    fn activated_entry(ctx: &AppContext, plugin: &str) -> Option<ActivationEntry> {
        let settings = load_settings(ctx, Scope::Global).unwrap();
        settings.activated.get(plugin).cloned()
    }

    #[test]
    fn activate_bare_plugin_name_activates_all_commands() {
        let dir = tempfile::TempDir::new().unwrap();
        make_plugin_dir(dir.path(), "ask");
        let ctx = AppContext::for_test_with_creft_home(
            dir.path().to_path_buf(),
            dir.path().to_path_buf(),
        );

        activate(&ctx, "ask", Scope::Global).unwrap();

        assert_eq!(
            activated_entry(&ctx, "ask"),
            Some(ActivationEntry::All(true)),
        );
    }

    #[test]
    fn activate_qualified_owner_plugin_strips_owner_and_activates() {
        let dir = tempfile::TempDir::new().unwrap();
        // "creft" is not an installed plugin, but "ask" is.
        make_plugin_dir(dir.path(), "ask");
        let ctx = AppContext::for_test_with_creft_home(
            dir.path().to_path_buf(),
            dir.path().to_path_buf(),
        );

        activate(&ctx, "creft/ask", Scope::Global).unwrap();

        assert_eq!(
            activated_entry(&ctx, "ask"),
            Some(ActivationEntry::All(true)),
        );
        // The "creft" key must not appear in the activation map.
        assert!(activated_entry(&ctx, "creft").is_none());
    }

    #[test]
    fn activate_plugin_slash_cmd_activates_specific_command() {
        let dir = tempfile::TempDir::new().unwrap();
        // "ask" is installed and has a "fetch.md" skill file.
        make_plugin_skill(dir.path(), "ask", "fetch.md");
        let ctx = AppContext::for_test_with_creft_home(
            dir.path().to_path_buf(),
            dir.path().to_path_buf(),
        );

        activate(&ctx, "ask/fetch", Scope::Global).unwrap();

        assert_eq!(
            activated_entry(&ctx, "ask"),
            Some(ActivationEntry::Commands(vec!["fetch".to_string()])),
        );
    }

    #[test]
    fn activate_returns_package_not_found_when_neither_segment_is_installed() {
        let dir = tempfile::TempDir::new().unwrap();
        let ctx = AppContext::for_test_with_creft_home(
            dir.path().to_path_buf(),
            dir.path().to_path_buf(),
        );

        let err = activate(&ctx, "creft/ask", Scope::Global).unwrap_err();
        assert!(
            matches!(err, CreftError::PackageNotFound(ref name) if name == "creft"),
            "expected PackageNotFound(creft) when neither segment is installed; got: {err:?}",
        );
    }

    #[test]
    fn activate_prefers_literal_interpretation_when_first_segment_is_installed() {
        let dir = tempfile::TempDir::new().unwrap();
        // Both "creft" and "ask" are installed.
        make_plugin_skill(dir.path(), "creft", "ask.md");
        make_plugin_dir(dir.path(), "ask");
        let ctx = AppContext::for_test_with_creft_home(
            dir.path().to_path_buf(),
            dir.path().to_path_buf(),
        );

        // "creft/ask" with "creft" installed → literal: plugin=creft, cmd=ask.
        activate(&ctx, "creft/ask", Scope::Global).unwrap();

        assert_eq!(
            activated_entry(&ctx, "creft"),
            Some(ActivationEntry::Commands(vec!["ask".to_string()])),
        );
        // "ask" plugin must not be touched.
        assert!(activated_entry(&ctx, "ask").is_none());
    }
}
