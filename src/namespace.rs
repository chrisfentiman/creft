//! Shared namespace resolution and access control for creft runtime primitives.
//!
//! Every runtime primitive (search, store, cache, lock, events) needs the same
//! three operations:
//! - qualify a local name into a fully-qualified name,
//! - resolve a reference to a resource, and
//! - check whether the caller has access.
//!
//! This module provides those operations once. It has no dependencies on search,
//! runner, or any other creft subsystem — it operates on strings and returns strings.

use std::collections::HashSet;
use std::fmt;

/// Fully qualify a local resource name using the caller's namespace and plugin context.
///
/// Skill authors use local names: `creft_index("beta", content)`. This function
/// produces the fully-qualified name used internally for storage and lookup.
///
/// Examples:
/// - `qualify("beta", "deploy", None)`          -> `"deploy.beta"`
/// - `qualify("beta", "deploy", Some("acme"))` -> `"acme.deploy.beta"`
/// - `qualify("beta", "", None)`                -> `"beta"`
/// - `qualify("beta", "", Some("acme"))`        -> `"acme.beta"`
pub(crate) fn qualify(local_name: &str, namespace: &str, plugin: Option<&str>) -> String {
    match (plugin, namespace) {
        (Some(p), ns) if !ns.is_empty() => format!("{}.{}.{}", p, ns, local_name),
        (Some(p), _) => format!("{}.{}", p, local_name),
        (None, ns) if !ns.is_empty() => format!("{}.{}", ns, local_name),
        (None, _) => local_name.to_owned(),
    }
}

/// Resolve a resource reference from a caller's context.
///
/// If the name contains no dots, it is a local reference — qualify it using the
/// caller's namespace and plugin context and return it. Local references always
/// succeed (a skill can always access its own namespace's resources).
///
/// If the name contains dots, it is a cross-namespace reference. Look up the
/// fully-qualified name in the access registry. If the resource exists and is
/// marked global, return it. If not global or not found, return `Err(AccessError)`.
///
/// This mirrors creft's CLI command resolution: local names resolve implicitly,
/// cross-namespace names require explicit qualification and access permission.
pub(crate) fn resolve(
    name: &str,
    caller_namespace: &str,
    caller_plugin: Option<&str>,
    registry: &AccessRegistry,
) -> Result<String, AccessError> {
    if name.contains('.') {
        // Cross-namespace reference: must be registered as global.
        if registry.is_global(name) {
            Ok(name.to_owned())
        } else {
            Err(AccessError {
                requested: name.to_owned(),
                resolved: name.to_owned(),
            })
        }
    } else {
        // Local reference: always accessible.
        Ok(qualify(name, caller_namespace, caller_plugin))
    }
}

/// Extract the top-level namespace from a fully-qualified skill name.
///
/// The namespace is the first whitespace-delimited token of the skill name.
/// A single-token skill name has no namespace and returns `""`.
///
/// Examples:
/// - `"deploy rollback"` -> `"deploy"`
/// - `"aws s3 copy"`     -> `"aws"`
/// - `"hello"`           -> `""`
pub(crate) fn skill_namespace(skill_name: &str) -> &str {
    let mut parts = skill_name.splitn(2, char::is_whitespace);
    let first = parts.next().unwrap_or("");
    // A namespace only exists when there are at least two space-delimited tokens.
    if parts.next().is_some() { first } else { "" }
}

/// Access control registry for namespace-scoped resources.
///
/// Tracks which fully-qualified resource names are marked global. Resources not
/// in the registry are assumed to be namespace-local (not global).
///
/// The registry is populated at two points:
/// - At index build time: disk-persisted indexes record their access
///   level in the index file metadata.
/// - At runtime: `creft_index` calls with `global: true` register the
///   resource as globally accessible for the duration of the skill execution.
#[derive(Debug, Default)]
pub(crate) struct AccessRegistry {
    global_names: HashSet<String>,
}

impl AccessRegistry {
    /// Create an empty registry. All resources default to namespace-local.
    pub fn new() -> Self {
        Self::default()
    }

    /// Mark a fully-qualified name as globally accessible.
    pub fn mark_global(&mut self, qualified_name: &str) {
        self.global_names.insert(qualified_name.to_owned());
    }

    /// Check whether a fully-qualified name is globally accessible.
    pub fn is_global(&self, qualified_name: &str) -> bool {
        self.global_names.contains(qualified_name)
    }
}

/// Error returned when a cross-namespace access is denied.
///
/// The channel handler converts this into an error message in the search
/// response JSON so the skill author receives a clear diagnostic.
#[derive(Debug)]
pub(crate) struct AccessError {
    /// The name the caller tried to access.
    pub requested: String,
    /// The fully-qualified name it resolved to (same as `requested` for cross-namespace references).
    pub resolved: String,
}

impl fmt::Display for AccessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "access denied: '{}' is not shared globally",
            self.resolved
        )
    }
}

impl std::error::Error for AccessError {}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use rstest::rstest;

    // ── qualify ───────────────────────────────────────────────────────────────

    #[rstest]
    #[case::named_namespace("beta", "deploy", None, "deploy.beta")]
    #[case::named_namespace_with_plugin("beta", "deploy", Some("acme"), "acme.deploy.beta")]
    #[case::root_namespace("beta", "", None, "beta")]
    #[case::root_namespace_with_plugin("beta", "", Some("acme"), "acme.beta")]
    fn qualify_produces_correct_qualified_name(
        #[case] local_name: &str,
        #[case] namespace: &str,
        #[case] plugin: Option<&str>,
        #[case] expected: &str,
    ) {
        assert_eq!(qualify(local_name, namespace, plugin), expected);
    }

    // ── skill_namespace ───────────────────────────────────────────────────────

    #[rstest]
    #[case::two_part_name("deploy rollback", "deploy")]
    #[case::single_token("hello", "")]
    #[case::three_part_name("aws s3 copy", "aws")]
    #[case::empty_string("", "")]
    fn skill_namespace_extracts_top_level_namespace(
        #[case] skill_name: &str,
        #[case] expected: &str,
    ) {
        assert_eq!(skill_namespace(skill_name), expected);
    }

    // ── resolve — local references ────────────────────────────────────────────

    #[test]
    fn resolve_local_name_qualifies_without_registry_check() {
        let registry = AccessRegistry::new();
        let result = resolve("beta", "deploy", None, &registry).unwrap();
        assert_eq!(result, "deploy.beta");
    }

    #[test]
    fn resolve_local_name_with_plugin_qualifies_correctly() {
        let registry = AccessRegistry::new();
        let result = resolve("beta", "deploy", Some("acme"), &registry).unwrap();
        assert_eq!(result, "acme.deploy.beta");
    }

    // ── resolve — cross-namespace references ──────────────────────────────────

    #[test]
    fn resolve_cross_namespace_name_succeeds_when_global() {
        let mut registry = AccessRegistry::new();
        registry.mark_global("deploy.configs");
        let result = resolve("deploy.configs", "test", None, &registry).unwrap();
        assert_eq!(result, "deploy.configs");
    }

    #[test]
    fn resolve_cross_namespace_name_denied_when_not_registered() {
        let registry = AccessRegistry::new();
        let err = resolve("deploy.configs", "test", None, &registry).unwrap_err();
        assert_eq!(err.requested, "deploy.configs");
        assert_eq!(err.resolved, "deploy.configs");
    }

    #[test]
    fn resolve_cross_namespace_name_denied_when_not_global() {
        // Name exists in registry logic only when explicitly marked global;
        // an unregistered name is equivalent to a non-global one.
        let registry = AccessRegistry::new();
        assert!(resolve("deploy.configs", "test", None, &registry).is_err());
    }

    // ── AccessError display ───────────────────────────────────────────────────

    #[test]
    fn access_error_display_includes_resource_name() {
        let err = AccessError {
            requested: "deploy.configs".to_owned(),
            resolved: "deploy.configs".to_owned(),
        };
        assert_eq!(
            err.to_string(),
            "access denied: 'deploy.configs' is not shared globally"
        );
    }

    // ── AccessRegistry ────────────────────────────────────────────────────────

    #[test]
    fn access_registry_starts_empty() {
        let registry = AccessRegistry::new();
        assert!(!registry.is_global("anything"));
    }

    #[test]
    fn access_registry_mark_and_check_global() {
        let mut registry = AccessRegistry::new();
        registry.mark_global("deploy.configs");
        assert!(registry.is_global("deploy.configs"));
        assert!(!registry.is_global("deploy.other"));
    }

    #[test]
    fn access_registry_default_is_empty() {
        let registry = AccessRegistry::default();
        assert!(!registry.is_global("anything"));
    }
}
