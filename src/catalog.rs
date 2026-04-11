use serde::Deserialize;

use crate::error::CreftError;

/// A catalog listing available plugins in a git repository.
///
/// Corresponds to `.creft/catalog.json` at the repository root.
#[derive(Debug, Clone, Deserialize)]
pub struct Catalog {
    /// Catalog identifier (kebab-case).
    #[allow(dead_code)] // used in Stage 5 search/display
    pub name: String,

    /// Human-readable description of this catalog.
    #[serde(default)]
    #[allow(dead_code)] // used in Stage 5 search/display
    pub description: String,

    /// Available plugins.
    pub plugins: Vec<CatalogEntry>,
}

/// A single plugin listed in a catalog.
#[derive(Debug, Clone, Deserialize)]
pub struct CatalogEntry {
    /// Plugin name (kebab-case, no whitespace).
    pub name: String,

    /// Where to fetch this plugin's files from.
    pub source: PluginSource,

    /// Brief description shown in search results.
    #[serde(default)]
    pub description: String,

    /// Plugin version string.
    #[serde(default)]
    pub version: Option<String>,

    /// Search tags for discovery.
    #[serde(default)]
    pub tags: Vec<String>,
}

/// Where a catalog entry's plugin can be fetched from.
///
/// A plain string is interpreted as a relative path within the catalog
/// repository. A JSON object with a `"type"` field is a `TypedSource`.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum PluginSource {
    /// Relative path within the catalog repository (e.g. `"./plugins/fetch"`).
    Path(String),

    /// Structured source with an explicit type field.
    #[allow(dead_code)] // consumed in Stage 5 when installing typed-source plugins
    Typed(TypedSource),
}

/// Explicit source type for plugins not co-located with the catalog.
///
/// Used in Stage 5 (search/discovery) for non-path plugin sources.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
#[allow(dead_code)] // fields consumed in Stage 5 install-from-typed-source
pub enum TypedSource {
    /// GitHub repository.
    #[serde(rename = "github")]
    GitHub {
        repo: String,
        #[serde(default)]
        r#ref: Option<String>,
    },

    /// Any git URL.
    #[serde(rename = "git")]
    Git {
        url: String,
        #[serde(default)]
        r#ref: Option<String>,
    },
}

/// Parse and validate a catalog from a JSON string.
///
/// `catalog_source` is a human-readable label included in parse error messages
/// so failures are diagnosable without inspecting raw bytes.
pub fn parse_catalog(content: &str, catalog_source: &str) -> Result<Catalog, CreftError> {
    // Parse the outer structure first to get plugin count and names for diagnostics.
    let catalog: serde_json::Value =
        serde_json::from_str(content).map_err(|e| CreftError::CatalogParse {
            catalog_source: catalog_source.to_string(),
            detail: e.to_string(),
        })?;

    // Validate the plugins array exists before deserializing individual entries.
    let plugins_value = catalog
        .get("plugins")
        .and_then(|v| v.as_array())
        .ok_or_else(|| CreftError::CatalogParse {
            catalog_source: catalog_source.to_string(),
            detail: "missing or invalid 'plugins' array".to_string(),
        })?;

    // Deserialize each entry individually so we can identify which one failed.
    let mut entries: Vec<CatalogEntry> = Vec::with_capacity(plugins_value.len());
    for (i, entry_value) in plugins_value.iter().enumerate() {
        let entry: CatalogEntry =
            serde_json::from_value(entry_value.clone()).map_err(|e| CreftError::CatalogParse {
                catalog_source: catalog_source.to_string(),
                detail: format!("plugin entry {i}: {e}"),
            })?;
        entries.push(entry);
    }

    // Reject empty plugin names.
    for entry in &entries {
        if entry.name.is_empty() {
            return Err(CreftError::CatalogParse {
                catalog_source: catalog_source.to_string(),
                detail: "plugin entry has empty name".to_string(),
            });
        }
    }

    // Reject duplicate plugin names.
    let mut seen = std::collections::HashSet::new();
    for entry in &entries {
        if !seen.insert(&entry.name) {
            return Err(CreftError::CatalogParse {
                catalog_source: catalog_source.to_string(),
                detail: format!("duplicate plugin name: '{}'", entry.name),
            });
        }
    }

    // Deserialize the full catalog struct now that entries are validated.
    let mut result: Catalog =
        serde_json::from_str(content).map_err(|e| CreftError::CatalogParse {
            catalog_source: catalog_source.to_string(),
            detail: e.to_string(),
        })?;
    result.plugins = entries;

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use rstest::rstest;

    fn minimal_catalog(plugins_json: &str) -> String {
        format!(r#"{{"name":"test","description":"Test catalog","plugins":{plugins_json}}}"#)
    }

    // --- parse_catalog: valid inputs ---

    #[test]
    fn parse_catalog_single_path_source() {
        let json = minimal_catalog(
            r#"[{"name":"fetch","source":"./plugins/fetch","description":"Fetch dep source","version":"0.1.0","tags":["research"]}]"#,
        );
        let catalog = parse_catalog(&json, "test").unwrap();
        assert_eq!(catalog.name, "test");
        assert_eq!(catalog.plugins.len(), 1);
        let entry = &catalog.plugins[0];
        assert_eq!(entry.name, "fetch");
        assert_eq!(entry.description, "Fetch dep source");
        assert_eq!(entry.version, Some("0.1.0".to_string()));
        assert_eq!(entry.tags, vec!["research"]);
        assert!(matches!(entry.source, PluginSource::Path(ref p) if p == "./plugins/fetch"));
    }

    #[test]
    fn parse_catalog_typed_github_source() {
        let json = minimal_catalog(
            r#"[{"name":"my-plugin","source":{"type":"github","repo":"owner/my-plugin"}}]"#,
        );
        let catalog = parse_catalog(&json, "test").unwrap();
        let entry = &catalog.plugins[0];
        assert!(matches!(
            entry.source,
            PluginSource::Typed(TypedSource::GitHub { ref repo, .. }) if repo == "owner/my-plugin"
        ));
    }

    #[test]
    fn parse_catalog_typed_git_source() {
        let json = minimal_catalog(
            r#"[{"name":"my-plugin","source":{"type":"git","url":"https://example.com/repo.git"}}]"#,
        );
        let catalog = parse_catalog(&json, "test").unwrap();
        let entry = &catalog.plugins[0];
        assert!(matches!(
            entry.source,
            PluginSource::Typed(TypedSource::Git { ref url, .. }) if url == "https://example.com/repo.git"
        ));
    }

    #[test]
    fn parse_catalog_optional_fields_default() {
        let json = minimal_catalog(r#"[{"name":"simple","source":"./simple"}]"#);
        let catalog = parse_catalog(&json, "test").unwrap();
        let entry = &catalog.plugins[0];
        assert_eq!(entry.description, "");
        assert_eq!(entry.version, None);
        assert!(entry.tags.is_empty());
    }

    #[test]
    fn parse_catalog_multiple_plugins() {
        let json = minimal_catalog(
            r#"[{"name":"alpha","source":"./alpha"},{"name":"beta","source":"./beta"}]"#,
        );
        let catalog = parse_catalog(&json, "test").unwrap();
        assert_eq!(catalog.plugins.len(), 2);
        assert_eq!(catalog.plugins[0].name, "alpha");
        assert_eq!(catalog.plugins[1].name, "beta");
    }

    #[test]
    fn parse_catalog_empty_plugins_array() {
        let json = minimal_catalog("[]");
        let catalog = parse_catalog(&json, "test").unwrap();
        assert!(catalog.plugins.is_empty());
    }

    // --- parse_catalog: error cases ---

    #[test]
    fn parse_catalog_malformed_json_returns_catalog_parse_error() {
        let result = parse_catalog("not json", "bad-source");
        assert!(
            matches!(result, Err(CreftError::CatalogParse { ref catalog_source, .. }) if catalog_source == "bad-source")
        );
    }

    #[test]
    fn parse_catalog_missing_plugins_key() {
        let result = parse_catalog(r#"{"name":"test"}"#, "test-catalog");
        let err = result.unwrap_err();
        assert!(matches!(err, CreftError::CatalogParse { .. }));
        assert!(err.to_string().contains("plugins"));
    }

    #[test]
    fn parse_catalog_empty_plugin_name_rejected() {
        let json = minimal_catalog(r#"[{"name":"","source":"./x"}]"#);
        let err = parse_catalog(&json, "test").unwrap_err();
        assert!(matches!(err, CreftError::CatalogParse { .. }));
        assert!(err.to_string().contains("empty name"));
    }

    #[test]
    fn parse_catalog_duplicate_plugin_names_rejected() {
        let json =
            minimal_catalog(r#"[{"name":"fetch","source":"./a"},{"name":"fetch","source":"./b"}]"#);
        let err = parse_catalog(&json, "test").unwrap_err();
        assert!(matches!(err, CreftError::CatalogParse { .. }));
        assert!(err.to_string().contains("duplicate"));
    }

    #[rstest]
    #[case::missing_source(r#"[{"name":"fetch"}]"#, "missing field")]
    #[case::bad_typed_source(
        r#"[{"name":"fetch","source":{"type":"unknown","url":"x"}}]"#,
        "plugin entry 0"
    )]
    fn parse_catalog_malformed_entry(#[case] plugins_json: &str, #[case] _expected_hint: &str) {
        let json = minimal_catalog(plugins_json);
        let result = parse_catalog(&json, "test");
        assert!(result.is_err(), "Expected error for: {plugins_json}");
    }

    #[test]
    fn parse_catalog_error_names_catalog_source() {
        let json = minimal_catalog(r#"[{"name":"","source":"./x"}]"#);
        let err = parse_catalog(&json, "https://github.com/owner/repo").unwrap_err();
        assert!(err.to_string().contains("https://github.com/owner/repo"));
    }
}
