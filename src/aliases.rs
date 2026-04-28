//! Namespace alias data model and storage for creft.
//!
//! Aliases rewrite namespace prefixes at dispatch time so that `creft bl list`
//! resolves to the same skill as `creft backlog list` without renaming any files.
//!
//! Aliases are stored in `<scope_root>/aliases.yaml`. The file is optional; a
//! missing file is treated identically to an empty alias map.

use yaml_rust2::Yaml;
use yaml_rust2::yaml::Hash;

use crate::error::CreftError;
use crate::model::{AppContext, Scope};
use crate::store::validate_path_token;
use crate::yaml::{self, FromYaml, ToYaml, YamlError, emit_quoted, needs_quoting};

/// A single alias entry: `from` rewrites to `to` as a path-segment prefix.
///
/// Both `from` and `to` are non-empty `Vec<String>` of validated path tokens.
/// The vector representation, rather than a space-joined `String`, is the
/// canonical form: rewrite is a segment-wise prefix match, so storing the
/// segments avoids re-splitting on every rewrite.
///
/// The fields are `pub(crate)` instead of `pub` so the only construction
/// path is `Alias::new`. This single-construction-path discipline is what
/// makes `validate_path_token` truly single-sourced: every `Alias` value
/// in memory has been validated, regardless of which call site produced it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Alias {
    pub(crate) from: Vec<String>,
    pub(crate) to: Vec<String>,
}

impl Alias {
    /// Construct a validated alias from segment vectors.
    ///
    /// Returns `CreftError::MissingArg` if either vector is empty. Returns
    /// `CreftError::InvalidName` if any segment fails `store::validate_path_token`
    /// (empty, `.`, `..`, `/`, `\`).
    ///
    /// This is the only public path to construct an `Alias`. Both
    /// `Alias::from_yaml` and `cmd_alias_add` route through here so the
    /// validation rule is enforced by the type, not by every caller
    /// remembering to call `validate_path_token`.
    pub(crate) fn new(from: Vec<String>, to: Vec<String>) -> Result<Self, CreftError> {
        if from.is_empty() {
            return Err(CreftError::MissingArg("<from>".into()));
        }
        if to.is_empty() {
            return Err(CreftError::MissingArg("<to>".into()));
        }
        for segment in from.iter().chain(to.iter()) {
            validate_path_token(segment)?;
        }
        Ok(Alias { from, to })
    }

    /// Read-only access to the `from` segments. The field is non-public so
    /// external callers cannot bypass `Alias::new`'s validation.
    #[allow(dead_code)]
    pub(crate) fn from(&self) -> &[String] {
        &self.from
    }

    /// Read-only access to the `to` segments.
    #[allow(dead_code)]
    pub(crate) fn to(&self) -> &[String] {
        &self.to
    }
}

/// In-memory representation of a single scope's `aliases.yaml`.
///
/// Insertion order is preserved on round-trip. Order does not affect
/// rewrite (longest-match wins), but a stable order makes hand-edits
/// reviewable in version control.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AliasFile {
    pub aliases: Vec<Alias>,
}

impl FromYaml for Alias {
    fn from_yaml(yaml: &Yaml) -> Result<Self, YamlError> {
        let map: &Hash = yaml.as_hash().ok_or(YamlError::NotAMapping)?;
        // read_field enforces YAML shape only: the field exists, is a string,
        // is not whitespace-only. Per-token validation lives in Alias::new,
        // which both this path and cmd_alias_add call — keeping the rule in
        // exactly one place.
        let from_raw = read_field(map, "from")?;
        let to_raw = read_field(map, "to")?;
        let from: Vec<String> = from_raw.split_whitespace().map(str::to_string).collect();
        let to: Vec<String> = to_raw.split_whitespace().map(str::to_string).collect();
        Alias::new(from, to).map_err(alias_construction_to_yaml_error)
    }
}

/// Translate `Alias::new` failures to `YamlError`.
///
/// `Alias::new` only emits `MissingArg` (empty vector, which after
/// `split_whitespace` means a whitespace-only input) or `InvalidName`
/// (failed `validate_path_token`). Any other variant reaching this
/// function indicates a contract change that was not reflected here.
fn alias_construction_to_yaml_error(e: CreftError) -> YamlError {
    match e {
        CreftError::InvalidName(_) => YamlError::TypeError {
            field: "from/to",
            expected: "valid path token",
        },
        CreftError::MissingArg(ref field) if field.contains("from") => {
            YamlError::MissingField("from")
        }
        CreftError::MissingArg(_) => YamlError::MissingField("to"),
        // Alias::new does not produce any other variant. A change to
        // Alias::new that introduces a new failure mode without updating
        // this match is a contract change that must be revisited.
        other => unreachable!("Alias::new produced unexpected error: {other:?}"),
    }
}

impl FromYaml for AliasFile {
    fn from_yaml(yaml: &Yaml) -> Result<Self, YamlError> {
        match yaml {
            // An empty or null document is the same as an empty alias list.
            Yaml::Null => Ok(AliasFile::default()),
            Yaml::Array(items) => {
                let aliases = items
                    .iter()
                    .map(Alias::from_yaml)
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(AliasFile { aliases })
            }
            _ => Err(YamlError::NotAMapping),
        }
    }
}

impl ToYaml for AliasFile {
    fn to_yaml(&self, out: &mut String) {
        for alias in &self.aliases {
            // Block-list entry. Both fields emit as bare scalars when safe,
            // double-quoted via emit_quoted when needs_quoting reports true.
            out.push_str("- from: ");
            push_scalar(out, &alias.from.join(" "));
            out.push('\n');
            out.push_str("  to: ");
            push_scalar(out, &alias.to.join(" "));
            out.push('\n');
        }
    }
}

fn push_scalar(out: &mut String, value: &str) {
    if needs_quoting(value) {
        emit_quoted(out, value);
    } else {
        out.push_str(value);
    }
}

/// Read a `from` or `to` field from a YAML map as a non-empty string.
///
/// Enforces YAML shape only (the field exists, is a string, is not
/// whitespace-only). Per-token validation is the responsibility of `Alias::new`,
/// called by both this loader and `cmd_alias_add`. Splitting the responsibilities
/// this way keeps the path-token rule in exactly one place — `Alias::new`.
fn read_field(map: &Hash, field: &'static str) -> Result<String, YamlError> {
    match map.get(&Yaml::String(field.to_string())) {
        Some(Yaml::String(s)) if !s.trim().is_empty() => Ok(s.clone()),
        Some(Yaml::String(_)) | Some(Yaml::Null) | None => Err(YamlError::MissingField(field)),
        Some(_) => Err(YamlError::TypeError {
            field,
            expected: "string",
        }),
    }
}

/// Load `aliases.yaml` for the given scope.
///
/// Returns an empty `AliasFile` when the file does not exist or is zero bytes.
/// Returns `CreftError::Frontmatter` when the file exists, is non-empty, but
/// cannot be parsed; the path is included in the error message so the user can
/// locate and fix the file.
#[allow(dead_code)]
pub fn load_for_scope(ctx: &AppContext, scope: Scope) -> Result<AliasFile, CreftError> {
    let path = ctx.aliases_path_for(scope)?;
    if !path.exists() {
        return Ok(AliasFile::default());
    }
    let content = std::fs::read_to_string(&path)?;
    if content.is_empty() {
        return Ok(AliasFile::default());
    }
    yaml::from_str::<AliasFile>(&content)
        .map_err(|e| CreftError::Frontmatter(format!("{}: {}", path.display(), e)))
}

/// Save `aliases.yaml` for the given scope, overwriting any existing content.
///
/// Creates `<scope_root>/` if it does not exist. An empty `AliasFile` produces
/// a zero-byte file, which `load_for_scope` reads back as `AliasFile::default()`.
#[allow(dead_code)]
pub fn save_for_scope(ctx: &AppContext, scope: Scope, file: &AliasFile) -> Result<(), CreftError> {
    let path = ctx.aliases_path_for(scope)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let content = yaml::to_string(file);
    std::fs::write(&path, content)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use rstest::rstest;
    use tempfile::tempdir;

    #[test]
    fn alias_new_valid_single_segment() {
        let alias = Alias::new(vec!["bl".into()], vec!["backlog".into()]);
        assert!(alias.is_ok(), "simple single-segment alias must succeed");
    }

    #[test]
    fn alias_new_valid_multi_segment() {
        let alias = Alias::new(
            vec!["my".into(), "new".into()],
            vec!["foo".into(), "bar".into()],
        );
        assert!(alias.is_ok(), "multi-segment alias must succeed");
    }

    #[rstest]
    #[case::dotdot_in_from(vec!["..".into()], vec!["backlog".into()])]
    #[case::slash_in_to(vec!["bl".into()], vec!["a/b".into()])]
    #[case::dot_segment(vec![".".into()], vec!["backlog".into()])]
    #[case::backslash_segment(vec!["a\\b".into()], vec!["backlog".into()])]
    fn alias_new_rejects_invalid_segment(#[case] from: Vec<String>, #[case] to: Vec<String>) {
        let err = Alias::new(from, to).expect_err("invalid path segment must be rejected");
        assert!(matches!(err, CreftError::InvalidName(_)));
    }

    #[test]
    fn alias_new_rejects_empty_from() {
        let err =
            Alias::new(vec![], vec!["backlog".into()]).expect_err("empty from must be rejected");
        assert!(matches!(err, CreftError::MissingArg(_)));
    }

    #[test]
    fn alias_new_rejects_empty_to() {
        let err = Alias::new(vec!["bl".into()], vec![]).expect_err("empty to must be rejected");
        assert!(matches!(err, CreftError::MissingArg(_)));
    }

    #[test]
    fn alias_file_round_trips() {
        let original = AliasFile {
            aliases: vec![
                Alias::new(vec!["bl".into()], vec!["backlog".into()]).unwrap(),
                Alias::new(
                    vec!["my".into(), "new".into()],
                    vec!["foo".into(), "bar".into()],
                )
                .unwrap(),
            ],
        };
        let yaml_text = yaml::to_string(&original);
        let parsed: AliasFile =
            yaml::from_str(&yaml_text).expect("round-trip must parse without error");
        assert_eq!(original, parsed);
    }

    #[test]
    fn load_missing_file_returns_empty() {
        let dir = tempdir().unwrap();
        let ctx =
            crate::model::AppContext::for_test(dir.path().to_path_buf(), dir.path().to_path_buf());
        let result =
            load_for_scope(&ctx, Scope::Global).expect("missing aliases.yaml must not error");
        assert_eq!(result, AliasFile::default());
    }

    #[test]
    fn save_empty_writes_zero_bytes_and_loads_back_as_empty() {
        let dir = tempdir().unwrap();
        // Simulate the global root: the global scope resolves to `~/.creft/`,
        // so point HOME at `dir` so `resolve_root(Global)` → `dir/.creft/`.
        let creft_dir = dir.path().join(".creft");
        std::fs::create_dir_all(&creft_dir).unwrap();
        let ctx =
            crate::model::AppContext::for_test(dir.path().to_path_buf(), dir.path().to_path_buf());

        let empty = AliasFile::default();
        save_for_scope(&ctx, Scope::Global, &empty).expect("save must succeed");

        let alias_path = creft_dir.join("aliases.yaml");
        let bytes = std::fs::read(&alias_path).expect("aliases.yaml must exist after save");
        assert_eq!(
            bytes.len(),
            0,
            "empty AliasFile must produce a zero-byte file"
        );

        let loaded =
            load_for_scope(&ctx, Scope::Global).expect("loading zero-byte file must succeed");
        assert_eq!(loaded, AliasFile::default());
    }

    #[test]
    fn load_malformed_yaml_returns_frontmatter_error_with_path() {
        let dir = tempdir().unwrap();
        let creft_dir = dir.path().join(".creft");
        std::fs::create_dir_all(&creft_dir).unwrap();
        std::fs::write(creft_dir.join("aliases.yaml"), b"not: a: list:").unwrap();

        let ctx =
            crate::model::AppContext::for_test(dir.path().to_path_buf(), dir.path().to_path_buf());
        let err =
            load_for_scope(&ctx, Scope::Global).expect_err("malformed YAML must produce an error");
        match &err {
            CreftError::Frontmatter(msg) => {
                assert!(
                    msg.contains("aliases.yaml"),
                    "error message must contain the file name; got: {msg}"
                );
            }
            other => panic!("expected CreftError::Frontmatter, got {other:?}"),
        }
    }

    #[test]
    fn load_entry_with_slash_in_from_produces_frontmatter_error() {
        let dir = tempdir().unwrap();
        let creft_dir = dir.path().join(".creft");
        std::fs::create_dir_all(&creft_dir).unwrap();
        std::fs::write(
            creft_dir.join("aliases.yaml"),
            b"- from: my/skill\n  to: backlog\n",
        )
        .unwrap();

        let ctx =
            crate::model::AppContext::for_test(dir.path().to_path_buf(), dir.path().to_path_buf());
        let err = load_for_scope(&ctx, Scope::Global)
            .expect_err("invalid token in from field must produce an error");
        assert!(matches!(err, CreftError::Frontmatter(_)));
    }

    #[test]
    fn alias_new_accepts_at_and_exclamation() {
        // validate_path_token is the dispatch-time rule, not the stricter add-time rule.
        // Characters like @ and ! are not excluded by validate_path_token.
        let result = Alias::new(vec!["bl@!".into()], vec!["backlog".into()]);
        assert!(
            result.is_ok(),
            "dispatch-time validation must not exclude @ or !"
        );
    }

    #[test]
    fn load_whitespace_only_from_produces_error() {
        let dir = tempdir().unwrap();
        let creft_dir = dir.path().join(".creft");
        std::fs::create_dir_all(&creft_dir).unwrap();
        std::fs::write(
            creft_dir.join("aliases.yaml"),
            b"- from: \"   \"\n  to: backlog\n",
        )
        .unwrap();

        let ctx =
            crate::model::AppContext::for_test(dir.path().to_path_buf(), dir.path().to_path_buf());
        let err = load_for_scope(&ctx, Scope::Global)
            .expect_err("whitespace-only from must produce an error");
        assert!(matches!(err, CreftError::Frontmatter(_)));
    }

    #[test]
    fn double_space_in_from_normalizes_to_single_space_on_save() {
        let dir = tempdir().unwrap();
        let creft_dir = dir.path().join(".creft");
        std::fs::create_dir_all(&creft_dir).unwrap();
        // Write a YAML file with double-space between the two "from" tokens.
        std::fs::write(
            creft_dir.join("aliases.yaml"),
            b"- from: \"bl  backlog\"\n  to: foo\n",
        )
        .unwrap();

        let ctx =
            crate::model::AppContext::for_test(dir.path().to_path_buf(), dir.path().to_path_buf());
        let loaded = load_for_scope(&ctx, Scope::Global)
            .expect("double-space in from must parse as two segments");
        assert_eq!(
            loaded.aliases[0].from,
            vec!["bl".to_string(), "backlog".to_string()],
            "double-space must collapse to two segments"
        );

        // Now save and read the raw YAML back — it must use single space.
        save_for_scope(&ctx, Scope::Global, &loaded).unwrap();
        let saved_bytes = std::fs::read(creft_dir.join("aliases.yaml")).unwrap();
        let saved_str = std::str::from_utf8(&saved_bytes).unwrap();
        assert!(
            saved_str.contains("from: bl backlog"),
            "saved YAML must use single space between segments; got:\n{saved_str}"
        );
    }

    #[test]
    fn boolean_keyword_from_round_trips_as_string() {
        // 'true' would be parsed as Yaml::Boolean(true) if not quoted.
        // The emitter must double-quote it; the loader must read it back as string.
        let original = AliasFile {
            aliases: vec![Alias::new(vec!["true".into()], vec!["backlog".into()]).unwrap()],
        };
        let yaml_text = yaml::to_string(&original);
        assert!(
            yaml_text.contains("\"true\""),
            "boolean keyword 'true' must be double-quoted in emitted YAML; got:\n{yaml_text}"
        );
        let parsed: AliasFile =
            yaml::from_str(&yaml_text).expect("quoted 'true' must parse back as string alias");
        assert_eq!(original, parsed);
    }

    #[test]
    fn alias_errors_have_correct_exit_codes() {
        use crate::error::CreftError;
        assert_eq!(CreftError::AliasNotFound("x".into()).exit_code(), 2);
        assert_eq!(CreftError::AliasTargetNotFound("x".into()).exit_code(), 2);
        assert_eq!(
            CreftError::AliasConflict {
                from: "x".into(),
                kind: "built-in command".into()
            }
            .exit_code(),
            3
        );
        assert_eq!(CreftError::AliasCycle("x".into()).exit_code(), 3);
        assert_eq!(CreftError::InvalidName("bad/name".into()).exit_code(), 3);
    }
}
