//! YAML extraction and emission for creft's frontmatter types.
//!
//! Provides [`FromYaml`] and [`ToYaml`] traits with hand-written implementations
//! for the six types creft parses from YAML. Uses `yaml-rust2` for parsing and
//! writes YAML directly via string operations for emission.

use yaml_rust2::yaml::Hash;
use yaml_rust2::{Yaml, YamlLoader};

use crate::model::{Arg, CommandDef, EnvVar, Flag, LlmConfig};
use crate::registry::PackageManifest;

// ── Error type ────────────────────────────────────────────────────────────────

/// Errors produced during YAML parsing or extraction.
#[derive(Debug, thiserror::Error)]
pub enum YamlError {
    /// The YAML text could not be parsed (syntax error, includes line/col info).
    #[error("{0}")]
    Scan(String),
    /// A required field was absent or null in the YAML mapping.
    #[error("missing required field: {0}")]
    MissingField(&'static str),
    /// A field had an unexpected type (e.g., expected string, got array).
    #[error("field '{field}': expected {expected}")]
    TypeError {
        field: &'static str,
        expected: &'static str,
    },
    /// The top-level YAML document was not a mapping.
    #[error("expected a YAML mapping")]
    NotAMapping,
}

// ── Traits ────────────────────────────────────────────────────────────────────

/// Extract a typed value from a parsed `Yaml` node.
pub trait FromYaml: Sized {
    fn from_yaml(yaml: &Yaml) -> Result<Self, YamlError>;
}

/// Emit a value as YAML text, writing into an existing `String`.
pub trait ToYaml {
    fn to_yaml(&self, out: &mut String);
}

// ── Public convenience functions ─────────────────────────────────────────────

/// Parse a YAML string into a typed value.
///
/// Loads the first YAML document from `s`. Treats an empty or absent document
/// as `Yaml::Null`, which fails with [`YamlError::NotAMapping`] for struct
/// types — matching the behavior of the previous serde bridge.
pub fn from_str<T: FromYaml>(s: &str) -> Result<T, YamlError> {
    let docs = YamlLoader::load_from_str(s).map_err(|e| YamlError::Scan(e.to_string()))?;
    let doc = docs.into_iter().next().unwrap_or(Yaml::Null);
    T::from_yaml(&doc)
}

/// Serialize a value to a YAML string.
///
/// The output contains no `---` document marker. Callers that need frontmatter
/// delimiters add them around this string.
pub fn to_string<T: ToYaml>(value: &T) -> String {
    let mut out = String::new();
    value.to_yaml(&mut out);
    out
}

// ── Helper extractors ─────────────────────────────────────────────────────────

fn yaml_key(key: &str) -> Yaml {
    Yaml::String(key.to_string())
}

/// Extract a required string field. Returns `MissingField` when absent or null.
fn require_string(map: &Hash, field: &'static str) -> Result<String, YamlError> {
    match map.get(&yaml_key(field)) {
        Some(Yaml::String(s)) => Ok(s.clone()),
        Some(Yaml::Null) | None => Err(YamlError::MissingField(field)),
        Some(_) => Err(YamlError::TypeError {
            field,
            expected: "string",
        }),
    }
}

/// Extract an optional string field. Returns `None` when absent or null.
fn optional_string(map: &Hash, field: &'static str) -> Result<Option<String>, YamlError> {
    match map.get(&yaml_key(field)) {
        Some(Yaml::String(s)) => Ok(Some(s.clone())),
        Some(Yaml::Null) | None => Ok(None),
        Some(_) => Err(YamlError::TypeError {
            field,
            expected: "string",
        }),
    }
}

/// Extract a string field, returning `default` when absent or null.
fn string_or(map: &Hash, field: &'static str, default: &str) -> Result<String, YamlError> {
    match map.get(&yaml_key(field)) {
        Some(Yaml::String(s)) => Ok(s.clone()),
        Some(Yaml::Null) | None => Ok(default.to_string()),
        Some(_) => Err(YamlError::TypeError {
            field,
            expected: "string",
        }),
    }
}

/// Extract a bool field, returning `default` when absent or null.
fn bool_or(map: &Hash, field: &'static str, default: bool) -> Result<bool, YamlError> {
    match map.get(&yaml_key(field)) {
        Some(Yaml::Boolean(b)) => Ok(*b),
        Some(Yaml::Null) | None => Ok(default),
        Some(_) => Err(YamlError::TypeError {
            field,
            expected: "bool",
        }),
    }
}

/// Extract a `Vec<T>` from a YAML array field. Returns an empty vec when the
/// field is absent or null.
fn extract_vec<T: FromYaml>(map: &Hash, field: &'static str) -> Result<Vec<T>, YamlError> {
    match map.get(&yaml_key(field)) {
        Some(Yaml::Array(arr)) => arr.iter().map(T::from_yaml).collect(),
        Some(Yaml::Null) | None => Ok(Vec::new()),
        Some(_) => Err(YamlError::TypeError {
            field,
            expected: "array",
        }),
    }
}

/// Extract a `Vec<String>` from a YAML array field. Returns an empty vec when
/// the field is absent or null.
fn extract_string_vec(map: &Hash, field: &'static str) -> Result<Vec<String>, YamlError> {
    match map.get(&yaml_key(field)) {
        Some(Yaml::Array(arr)) => arr
            .iter()
            .map(|item| match item {
                Yaml::String(s) => Ok(s.clone()),
                _ => Err(YamlError::TypeError {
                    field,
                    expected: "string array",
                }),
            })
            .collect(),
        Some(Yaml::Null) | None => Ok(Vec::new()),
        Some(_) => Err(YamlError::TypeError {
            field,
            expected: "array",
        }),
    }
}

// ── FromYaml implementations ──────────────────────────────────────────────────

impl FromYaml for CommandDef {
    fn from_yaml(yaml: &Yaml) -> Result<Self, YamlError> {
        let map = yaml.as_hash().ok_or(YamlError::NotAMapping)?;
        Ok(CommandDef {
            name: require_string(map, "name")?,
            description: require_string(map, "description")?,
            args: extract_vec::<Arg>(map, "args")?,
            flags: extract_vec::<Flag>(map, "flags")?,
            env: extract_vec::<EnvVar>(map, "env")?,
            tags: extract_string_vec(map, "tags")?,
            supports: extract_string_vec(map, "supports")?,
        })
    }
}

impl FromYaml for Arg {
    fn from_yaml(yaml: &Yaml) -> Result<Self, YamlError> {
        let map = yaml.as_hash().ok_or(YamlError::NotAMapping)?;
        Ok(Arg {
            name: require_string(map, "name")?,
            description: string_or(map, "description", "")?,
            default: optional_string(map, "default")?,
            required: bool_or(map, "required", false)?,
            validation: optional_string(map, "validation")?,
        })
    }
}

impl FromYaml for Flag {
    fn from_yaml(yaml: &Yaml) -> Result<Self, YamlError> {
        let map = yaml.as_hash().ok_or(YamlError::NotAMapping)?;
        Ok(Flag {
            name: require_string(map, "name")?,
            short: optional_string(map, "short")?,
            description: string_or(map, "description", "")?,
            r#type: string_or(map, "type", "string")?,
            default: optional_string(map, "default")?,
            validation: optional_string(map, "validation")?,
        })
    }
}

impl FromYaml for EnvVar {
    fn from_yaml(yaml: &Yaml) -> Result<Self, YamlError> {
        let map = yaml.as_hash().ok_or(YamlError::NotAMapping)?;
        Ok(EnvVar {
            name: require_string(map, "name")?,
            required: bool_or(map, "required", true)?,
        })
    }
}

impl FromYaml for LlmConfig {
    fn from_yaml(yaml: &Yaml) -> Result<Self, YamlError> {
        let map = match yaml.as_hash() {
            Some(m) => m,
            // Treat a null/empty document as default LlmConfig.
            None if matches!(yaml, Yaml::Null) => return Ok(LlmConfig::default()),
            None => return Err(YamlError::NotAMapping),
        };
        Ok(LlmConfig {
            provider: string_or(map, "provider", "claude")?,
            model: string_or(map, "model", "")?,
            params: string_or(map, "params", "")?,
        })
    }
}

impl FromYaml for PackageManifest {
    fn from_yaml(yaml: &Yaml) -> Result<Self, YamlError> {
        let map = yaml.as_hash().ok_or(YamlError::NotAMapping)?;
        Ok(PackageManifest {
            name: require_string(map, "name")?,
            version: require_string(map, "version")?,
            description: require_string(map, "description")?,
            author: optional_string(map, "author")?,
            license: optional_string(map, "license")?,
        })
    }
}

// ── Emission helpers ──────────────────────────────────────────────────────────

/// Write a `key: value\n` line, quoting the value when needed.
fn emit_field(out: &mut String, key: &str, value: &str) {
    out.push_str(key);
    out.push_str(": ");
    if needs_quoting(value) {
        emit_quoted(out, value);
    } else {
        out.push_str(value);
    }
    out.push('\n');
}

/// Write a block-style string list:
/// ```yaml
/// key:
/// - value1
/// - value2
/// ```
fn emit_string_list(out: &mut String, key: &str, values: &[String]) {
    out.push_str(key);
    out.push_str(":\n");
    for v in values {
        out.push_str("- ");
        if needs_quoting(v) {
            emit_quoted(out, v);
        } else {
            out.push_str(v);
        }
        out.push('\n');
    }
}

/// Write a double-quoted YAML scalar, escaping `\`, `"`, and newlines.
fn emit_quoted(out: &mut String, s: &str) {
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            _ => out.push(ch),
        }
    }
    out.push('"');
}

/// Whether a YAML scalar value requires double-quoting to roundtrip safely.
///
/// Quotes when the value is empty, starts with a YAML indicator character,
/// contains `: ` or `#`, looks like a YAML number, or matches a YAML 1.1/1.2
/// keyword (`true`, `null`, `yes`, `.inf`, etc.).
fn needs_quoting(s: &str) -> bool {
    if s.is_empty() {
        return true;
    }
    if s.contains(": ")
        || s.contains('\n')
        || s.contains('#')
        || s.starts_with('\'')
        || s.starts_with('"')
        || s.starts_with('{')
        || s.starts_with('[')
        || s.starts_with('&')
        || s.starts_with('*')
        || s.starts_with('!')
        || s.starts_with('%')
        || s.starts_with('@')
        || s.starts_with('`')
        || s.starts_with("? ")
        || s.starts_with("> ")
        || s.starts_with("| ")
        || s.starts_with("- ")
        || matches!(s, "?" | ">" | "|" | "-")
    {
        return true;
    }
    if looks_like_yaml_number(s) {
        return true;
    }
    // YAML 1.1 and 1.2 boolean/null keywords. The emitter quotes these so that
    // a re-parse under either schema returns Yaml::String, not Yaml::Boolean or
    // Yaml::Null.
    matches!(
        s,
        "true"
            | "false"
            | "null"
            | "~"
            | "True"
            | "False"
            | "Null"
            | "TRUE"
            | "FALSE"
            | "NULL"
            | "yes"
            | "no"
            | "on"
            | "off"
            | "Yes"
            | "No"
            | "On"
            | "Off"
            | "YES"
            | "NO"
            | "ON"
            | "OFF"
            | ".inf"
            | ".Inf"
            | ".INF"
            | "-.inf"
            | "-.Inf"
            | "-.INF"
            | "+.inf"
            | "+.Inf"
            | "+.INF"
            | ".nan"
            | ".NaN"
            | ".NAN"
    )
}

/// Whether a string parses as a YAML 1.2 numeric scalar.
///
/// Conservative check: false positives add harmless quotes; false negatives
/// break roundtripping. Covers plain integers, hex/octal/binary literals,
/// floats, and signed variants.
fn looks_like_yaml_number(s: &str) -> bool {
    let bytes = s.as_bytes();
    if bytes.is_empty() {
        return false;
    }
    let first = bytes[0];
    if first.is_ascii_digit() {
        return true;
    }
    // `.5` — float with leading dot
    if first == b'.' && bytes.len() > 1 && bytes[1].is_ascii_digit() {
        return true;
    }
    // `+1`, `-3.14`, `+.5`, `-.5`
    if (first == b'+' || first == b'-') && bytes.len() > 1 {
        let second = bytes[1];
        if second.is_ascii_digit() || second == b'.' {
            return true;
        }
    }
    false
}

// ── ToYaml implementations ────────────────────────────────────────────────────

impl ToYaml for CommandDef {
    fn to_yaml(&self, out: &mut String) {
        emit_field(out, "name", &self.name);
        emit_field(out, "description", &self.description);
        if !self.args.is_empty() {
            out.push_str("args:\n");
            for arg in &self.args {
                out.push_str("- ");
                arg.emit_inline(out);
            }
        }
        if !self.flags.is_empty() {
            out.push_str("flags:\n");
            for flag in &self.flags {
                out.push_str("- ");
                flag.emit_inline(out);
            }
        }
        if !self.env.is_empty() {
            out.push_str("env:\n");
            for env_var in &self.env {
                out.push_str("- ");
                env_var.emit_inline(out);
            }
        }
        if !self.tags.is_empty() {
            emit_string_list(out, "tags", &self.tags);
        }
        if !self.supports.is_empty() {
            emit_string_list(out, "supports", &self.supports);
        }
    }
}

impl Arg {
    /// Emit an `Arg` as a block-sequence item. The first field is written on
    /// the same line as the `- ` prefix; subsequent fields are indented by 2.
    fn emit_inline(&self, out: &mut String) {
        emit_field(out, "name", &self.name);
        let indent = "  ";
        if !self.description.is_empty() {
            out.push_str(indent);
            emit_field(out, "description", &self.description);
        }
        if let Some(ref default) = self.default {
            out.push_str(indent);
            emit_field(out, "default", default);
        }
        if self.required {
            out.push_str(indent);
            out.push_str("required: true\n");
        }
        if let Some(ref validation) = self.validation {
            out.push_str(indent);
            emit_field(out, "validation", validation);
        }
    }
}

impl ToYaml for Arg {
    fn to_yaml(&self, out: &mut String) {
        self.emit_inline(out);
    }
}

impl Flag {
    fn emit_inline(&self, out: &mut String) {
        emit_field(out, "name", &self.name);
        let indent = "  ";
        if let Some(ref short) = self.short {
            out.push_str(indent);
            emit_field(out, "short", short);
        }
        if !self.description.is_empty() {
            out.push_str(indent);
            emit_field(out, "description", &self.description);
        }
        // Omit type when it is the default ("string") — matches skip_serializing_if behaviour.
        if self.r#type != "string" {
            out.push_str(indent);
            emit_field(out, "type", &self.r#type);
        }
        if let Some(ref default) = self.default {
            out.push_str(indent);
            emit_field(out, "default", default);
        }
        if let Some(ref validation) = self.validation {
            out.push_str(indent);
            emit_field(out, "validation", validation);
        }
    }
}

impl ToYaml for Flag {
    fn to_yaml(&self, out: &mut String) {
        self.emit_inline(out);
    }
}

impl EnvVar {
    fn emit_inline(&self, out: &mut String) {
        emit_field(out, "name", &self.name);
        // Omit `required` when true — that is the default, so skipping keeps
        // output minimal and matches the previous serde skip_serializing_if logic.
        if !self.required {
            out.push_str("  required: false\n");
        }
    }
}

impl ToYaml for EnvVar {
    fn to_yaml(&self, out: &mut String) {
        self.emit_inline(out);
    }
}

impl ToYaml for LlmConfig {
    fn to_yaml(&self, out: &mut String) {
        emit_field(out, "provider", &self.provider);
        if !self.model.is_empty() {
            emit_field(out, "model", &self.model);
        }
        if !self.params.is_empty() {
            emit_field(out, "params", &self.params);
        }
    }
}

impl ToYaml for PackageManifest {
    fn to_yaml(&self, out: &mut String) {
        emit_field(out, "name", &self.name);
        emit_field(out, "version", &self.version);
        emit_field(out, "description", &self.description);
        if let Some(ref author) = self.author {
            emit_field(out, "author", author);
        }
        if let Some(ref license) = self.license {
            emit_field(out, "license", license);
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use rstest::rstest;

    // ── CommandDef extraction ─────────────────────────────────────────────────

    #[test]
    fn command_def_required_fields_extracted() {
        let yaml = "name: hello\ndescription: greet";
        let def: CommandDef = from_str(yaml).unwrap();
        assert_eq!(def.name, "hello");
        assert_eq!(def.description, "greet");
        assert!(def.args.is_empty());
        assert!(def.flags.is_empty());
        assert!(def.env.is_empty());
        assert!(def.tags.is_empty());
        assert!(def.supports.is_empty());
    }

    #[test]
    fn command_def_missing_name_returns_missing_field_error() {
        let yaml = "description: greet";
        let err = from_str::<CommandDef>(yaml).unwrap_err();
        assert!(
            matches!(err, YamlError::MissingField("name")),
            "expected MissingField(\"name\"), got: {err}"
        );
    }

    #[test]
    fn command_def_missing_description_returns_missing_field_error() {
        let yaml = "name: hello";
        let err = from_str::<CommandDef>(yaml).unwrap_err();
        assert!(
            matches!(err, YamlError::MissingField("description")),
            "expected MissingField(\"description\"), got: {err}"
        );
    }

    #[test]
    fn command_def_name_wrong_type_returns_type_error() {
        let yaml = "name:\n  - 1\n  - 2\ndescription: greet";
        let err = from_str::<CommandDef>(yaml).unwrap_err();
        assert!(
            matches!(err, YamlError::TypeError { field: "name", .. }),
            "expected TypeError for 'name', got: {err}"
        );
    }

    #[test]
    fn command_def_unknown_fields_ignored() {
        let yaml = "name: hello\ndescription: greet\npipe: true\nlegacy_field: whatever";
        let def: CommandDef = from_str(yaml).unwrap();
        assert_eq!(def.name, "hello");
        assert_eq!(def.description, "greet");
    }

    #[test]
    fn command_def_not_a_mapping_returns_error() {
        let yaml = "- item1\n- item2";
        let err = from_str::<CommandDef>(yaml).unwrap_err();
        assert!(
            matches!(err, YamlError::NotAMapping),
            "expected NotAMapping, got: {err}"
        );
    }

    // ── Args extraction ───────────────────────────────────────────────────────

    #[test]
    fn command_def_with_arg_extracted() {
        let yaml = "name: greet\ndescription: say hi\nargs:\n- name: who\n  description: target\n";
        let def: CommandDef = from_str(yaml).unwrap();
        assert_eq!(def.args.len(), 1);
        assert_eq!(def.args[0].name, "who");
        assert_eq!(def.args[0].description, "target");
        assert!(!def.args[0].required);
        assert!(def.args[0].default.is_none());
        assert!(def.args[0].validation.is_none());
    }

    #[test]
    fn arg_required_default_is_false() {
        let yaml = "name: greet\ndescription: d\nargs:\n- name: who\n";
        let def: CommandDef = from_str(yaml).unwrap();
        assert!(!def.args[0].required);
    }

    #[test]
    fn arg_required_true_extracted() {
        let yaml = "name: greet\ndescription: d\nargs:\n- name: who\n  required: true\n";
        let def: CommandDef = from_str(yaml).unwrap();
        assert!(def.args[0].required);
    }

    #[test]
    fn arg_with_default_extracted() {
        let yaml = "name: g\ndescription: d\nargs:\n- name: x\n  default: world\n";
        let def: CommandDef = from_str(yaml).unwrap();
        assert_eq!(def.args[0].default, Some("world".to_string()));
    }

    #[test]
    fn arg_with_validation_extracted() {
        let yaml = "name: g\ndescription: d\nargs:\n- name: x\n  validation: ^[a-z]+$\n";
        let def: CommandDef = from_str(yaml).unwrap();
        assert_eq!(def.args[0].validation, Some("^[a-z]+$".to_string()));
    }

    // ── Flags extraction ──────────────────────────────────────────────────────

    #[test]
    fn flag_type_defaults_to_string_when_absent() {
        let yaml = "name: g\ndescription: d\nflags:\n- name: verbose\n";
        let def: CommandDef = from_str(yaml).unwrap();
        assert_eq!(def.flags[0].r#type, "string");
    }

    #[test]
    fn flag_bool_type_extracted() {
        let yaml = "name: g\ndescription: d\nflags:\n- name: verbose\n  type: bool\n";
        let def: CommandDef = from_str(yaml).unwrap();
        assert_eq!(def.flags[0].r#type, "bool");
    }

    #[test]
    fn flag_short_form_extracted() {
        let yaml = "name: g\ndescription: d\nflags:\n- name: verbose\n  short: v\n";
        let def: CommandDef = from_str(yaml).unwrap();
        assert_eq!(def.flags[0].short, Some("v".to_string()));
    }

    // ── EnvVar extraction ─────────────────────────────────────────────────────

    #[test]
    fn env_var_required_defaults_to_true() {
        let yaml = "name: g\ndescription: d\nenv:\n- name: TOKEN\n";
        let def: CommandDef = from_str(yaml).unwrap();
        assert!(def.env[0].required);
    }

    #[test]
    fn env_var_required_false_extracted() {
        let yaml = "name: g\ndescription: d\nenv:\n- name: TOKEN\n  required: false\n";
        let def: CommandDef = from_str(yaml).unwrap();
        assert!(!def.env[0].required);
    }

    // ── LlmConfig extraction ──────────────────────────────────────────────────

    #[test]
    fn llm_config_defaults_provider_to_claude() {
        let yaml = "{}";
        let cfg: LlmConfig = from_str(yaml).unwrap();
        assert_eq!(cfg.provider, "claude");
        assert!(cfg.model.is_empty());
        assert!(cfg.params.is_empty());
    }

    #[test]
    fn llm_config_empty_document_returns_default() {
        // Empty string → no YAML documents → Yaml::Null → default LlmConfig.
        let cfg: LlmConfig = from_str("").unwrap();
        assert_eq!(cfg.provider, "claude");
    }

    #[test]
    fn llm_config_provider_extracted() {
        let yaml = "provider: openai\nmodel: gpt-4o\nparams: --temperature 0.2";
        let cfg: LlmConfig = from_str(yaml).unwrap();
        assert_eq!(cfg.provider, "openai");
        assert_eq!(cfg.model, "gpt-4o");
        assert_eq!(cfg.params, "--temperature 0.2");
    }

    // ── PackageManifest extraction ─────────────────────────────────────────────

    #[test]
    fn package_manifest_all_fields_extracted() {
        let yaml = "name: my-pkg\nversion: 1.2.3\ndescription: A great package\nauthor: Alice\nlicense: MIT\n";
        let m: PackageManifest = from_str(yaml).unwrap();
        assert_eq!(m.name, "my-pkg");
        assert_eq!(m.version, "1.2.3");
        assert_eq!(m.description, "A great package");
        assert_eq!(m.author, Some("Alice".to_string()));
        assert_eq!(m.license, Some("MIT".to_string()));
    }

    #[test]
    fn package_manifest_optional_fields_absent() {
        let yaml = "name: my-pkg\nversion: 1.0.0\ndescription: Minimal\n";
        let m: PackageManifest = from_str(yaml).unwrap();
        assert!(m.author.is_none());
        assert!(m.license.is_none());
    }

    #[test]
    fn package_manifest_missing_name_returns_error() {
        let yaml = "version: 1.0.0\ndescription: No name here\n";
        let err = from_str::<PackageManifest>(yaml).unwrap_err();
        assert!(matches!(err, YamlError::MissingField("name")));
    }

    #[test]
    fn package_manifest_missing_version_returns_error() {
        let yaml = "name: pkg\ndescription: No version\n";
        let err = from_str::<PackageManifest>(yaml).unwrap_err();
        assert!(matches!(err, YamlError::MissingField("version")));
    }

    #[test]
    fn package_manifest_missing_description_returns_error() {
        let yaml = "name: pkg\nversion: 1.0.0\n";
        let err = from_str::<PackageManifest>(yaml).unwrap_err();
        assert!(matches!(err, YamlError::MissingField("description")));
    }

    // ── supports / tags extraction ────────────────────────────────────────────

    #[test]
    fn command_def_supports_extracted() {
        let yaml = "name: deploy\ndescription: deploy\nsupports:\n- dry-run\n";
        let def: CommandDef = from_str(yaml).unwrap();
        assert_eq!(def.supports, vec!["dry-run"]);
    }

    #[test]
    fn command_def_tags_extracted() {
        let yaml = "name: g\ndescription: d\ntags:\n- shell\n- utility\n";
        let def: CommandDef = from_str(yaml).unwrap();
        assert_eq!(def.tags, vec!["shell", "utility"]);
    }

    // ── Emission ──────────────────────────────────────────────────────────────

    #[test]
    fn command_def_minimal_emits_only_name_and_description() {
        let def = CommandDef {
            name: "hello".to_string(),
            description: "greet".to_string(),
            args: vec![],
            flags: vec![],
            env: vec![],
            tags: vec![],
            supports: vec![],
        };
        let out = to_string(&def);
        assert_eq!(out, "name: hello\ndescription: greet\n");
    }

    #[test]
    fn command_def_roundtrip_full() {
        let yaml = concat!(
            "name: deploy\n",
            "description: Deploy the app\n",
            "args:\n",
            "- name: env\n",
            "  description: target environment\n",
            "  required: true\n",
            "flags:\n",
            "- name: verbose\n",
            "  type: bool\n",
            "env:\n",
            "- name: TOKEN\n",
            "supports:\n",
            "- dry-run\n",
        );
        let def: CommandDef = from_str(yaml).unwrap();
        let emitted = to_string(&def);
        let def2: CommandDef = from_str(&emitted).unwrap();

        assert_eq!(def.name, def2.name);
        assert_eq!(def.description, def2.description);
        assert_eq!(def.args.len(), def2.args.len());
        assert_eq!(def.args[0].name, def2.args[0].name);
        assert_eq!(def.args[0].required, def2.args[0].required);
        assert_eq!(def.flags.len(), def2.flags.len());
        assert_eq!(def.flags[0].name, def2.flags[0].name);
        assert_eq!(def.flags[0].r#type, def2.flags[0].r#type);
        assert_eq!(def.env.len(), def2.env.len());
        assert_eq!(def.env[0].name, def2.env[0].name);
        assert_eq!(def.supports, def2.supports);
    }

    // ── Quoting ───────────────────────────────────────────────────────────────

    #[test]
    fn description_with_colon_space_is_quoted() {
        let def = CommandDef {
            name: "g".to_string(),
            description: "key: value style text".to_string(),
            args: vec![],
            flags: vec![],
            env: vec![],
            tags: vec![],
            supports: vec![],
        };
        let out = to_string(&def);
        assert!(
            out.contains("\"key: value style text\""),
            "description with ': ' should be quoted; got:\n{out}"
        );
        // Verify the emission re-parses cleanly.
        let def2: CommandDef = from_str(&out).unwrap();
        assert_eq!(def2.description, "key: value style text");
    }

    #[test]
    fn description_with_newline_is_quoted_and_roundtrips() {
        let def = CommandDef {
            name: "g".to_string(),
            description: "line one\nline two".to_string(),
            args: vec![],
            flags: vec![],
            env: vec![],
            tags: vec![],
            supports: vec![],
        };
        let out = to_string(&def);
        let def2: CommandDef = from_str(&out).unwrap();
        assert_eq!(def2.description, "line one\nline two");
    }

    #[rstest]
    #[case::plain_int("42")]
    #[case::float("3.14")]
    #[case::leading_dot(".5")]
    #[case::plus_int("+1")]
    #[case::neg_float("-3.14")]
    #[case::pos_inf(".inf")]
    #[case::neg_inf("-.inf")]
    #[case::nan(".nan")]
    fn numeric_string_description_roundtrips(#[case] value: &str) {
        let def = CommandDef {
            name: "g".to_string(),
            description: value.to_string(),
            args: vec![],
            flags: vec![],
            env: vec![],
            tags: vec![],
            supports: vec![],
        };
        let emitted = to_string(&def);
        // The emitter must quote the value so yaml-rust2 sees a String, not a number.
        let def2: CommandDef = from_str(&emitted).unwrap();
        assert_eq!(
            def2.description, value,
            "numeric-looking description '{value}' did not roundtrip; emission:\n{emitted}"
        );
    }

    #[rstest]
    #[case::question_mark_space("? something")]
    #[case::gt_space("> block")]
    #[case::pipe_space("| literal")]
    #[case::dash_space("- list item")]
    #[case::standalone_question("?")]
    #[case::standalone_gt(">")]
    #[case::standalone_pipe("|")]
    #[case::standalone_dash("-")]
    fn yaml_indicator_description_roundtrips(#[case] value: &str) {
        let def = CommandDef {
            name: "g".to_string(),
            description: value.to_string(),
            args: vec![],
            flags: vec![],
            env: vec![],
            tags: vec![],
            supports: vec![],
        };
        let emitted = to_string(&def);
        let def2: CommandDef = from_str(&emitted).unwrap();
        assert_eq!(
            def2.description, value,
            "YAML-indicator description '{value}' did not roundtrip; emission:\n{emitted}"
        );
    }

    #[rstest]
    #[case::true_lc("true")]
    #[case::false_lc("false")]
    #[case::null_lc("null")]
    #[case::yes_lc("yes")]
    #[case::no_lc("no")]
    #[case::on_lc("on")]
    #[case::off_lc("off")]
    fn yaml_keyword_description_roundtrips(#[case] value: &str) {
        let def = CommandDef {
            name: "g".to_string(),
            description: value.to_string(),
            args: vec![],
            flags: vec![],
            env: vec![],
            tags: vec![],
            supports: vec![],
        };
        let emitted = to_string(&def);
        let def2: CommandDef = from_str(&emitted).unwrap();
        assert_eq!(
            def2.description, value,
            "YAML keyword description '{value}' did not roundtrip; emission:\n{emitted}"
        );
    }

    // ── Tag / supports emission ───────────────────────────────────────────────

    #[test]
    fn command_def_supports_roundtrips() {
        let def = CommandDef {
            name: "deploy".to_string(),
            description: "Deploy".to_string(),
            args: vec![],
            flags: vec![],
            env: vec![],
            tags: vec![],
            supports: vec!["dry-run".to_string()],
        };
        let emitted = to_string(&def);
        let def2: CommandDef = from_str(&emitted).unwrap();
        assert_eq!(def2.supports, vec!["dry-run"]);
    }

    // ── EnvVar emission ───────────────────────────────────────────────────────

    #[test]
    fn env_var_required_true_omitted_from_emission() {
        let def = CommandDef {
            name: "g".to_string(),
            description: "d".to_string(),
            args: vec![],
            flags: vec![],
            env: vec![EnvVar {
                name: "TOKEN".to_string(),
                required: true,
            }],
            tags: vec![],
            supports: vec![],
        };
        let out = to_string(&def);
        // The `required: true` line is omitted since true is the default.
        assert!(
            !out.contains("required: true"),
            "required: true should be omitted from emission; got:\n{out}"
        );
        // The name must still appear.
        assert!(out.contains("name: TOKEN"));
    }

    #[test]
    fn env_var_required_false_is_explicit_in_emission() {
        let def = CommandDef {
            name: "g".to_string(),
            description: "d".to_string(),
            args: vec![],
            flags: vec![],
            env: vec![EnvVar {
                name: "OPTIONAL".to_string(),
                required: false,
            }],
            tags: vec![],
            supports: vec![],
        };
        let out = to_string(&def);
        assert!(
            out.contains("required: false"),
            "required: false should appear in emission; got:\n{out}"
        );
    }

    // ── Flag emission ─────────────────────────────────────────────────────────

    #[test]
    fn flag_default_type_omitted_from_emission() {
        let def = CommandDef {
            name: "g".to_string(),
            description: "d".to_string(),
            args: vec![],
            flags: vec![Flag {
                name: "output".to_string(),
                short: None,
                description: String::new(),
                r#type: "string".to_string(),
                default: None,
                validation: None,
            }],
            env: vec![],
            tags: vec![],
            supports: vec![],
        };
        let out = to_string(&def);
        // "string" is the default type — it should not appear in the output.
        assert!(
            !out.contains("type:"),
            "flag type 'string' should be omitted from emission; got:\n{out}"
        );
    }

    #[test]
    fn flag_bool_type_included_in_emission() {
        let def = CommandDef {
            name: "g".to_string(),
            description: "d".to_string(),
            args: vec![],
            flags: vec![Flag {
                name: "verbose".to_string(),
                short: Some("v".to_string()),
                description: "be verbose".to_string(),
                r#type: "bool".to_string(),
                default: None,
                validation: None,
            }],
            env: vec![],
            tags: vec![],
            supports: vec![],
        };
        let out = to_string(&def);
        assert!(
            out.contains("type: bool"),
            "bool type should appear; got:\n{out}"
        );
        assert!(out.contains("short: v"), "short should appear; got:\n{out}");
    }

    // ── needs_quoting unit checks ─────────────────────────────────────────────

    #[test]
    fn needs_quoting_empty_string() {
        assert!(needs_quoting(""));
    }

    #[test]
    fn needs_quoting_plain_identifier_is_false() {
        assert!(!needs_quoting("hello"));
        assert!(!needs_quoting("hello-world"));
        assert!(!needs_quoting("snake_case"));
    }

    #[rstest]
    #[case::hash("has#comment")]
    #[case::colon_space("key: val")]
    #[case::starts_brace("{key: val}")]
    #[case::starts_bracket("[1, 2]")]
    #[case::starts_single_quote("'quoted'")]
    #[case::starts_double_quote("\"quoted\"")]
    fn needs_quoting_special_chars(#[case] value: &str) {
        assert!(
            needs_quoting(value),
            "expected needs_quoting(\"{value}\") == true"
        );
    }
}
