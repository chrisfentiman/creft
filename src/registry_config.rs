//! Registry configuration resolution for dependency validation.
//!
//! Resolves registry URLs and authentication credentials from environment
//! variables and config files. Supports Python (PyPI/pip/uv) and npm
//! registry configurations.
//!
//! Design principles:
//! - Fail open: any config parse error falls back to public registry silently.
//! - Auth tokens are never logged or included in warning messages.
//! - Custom Debug impls redact secrets.

use std::collections::HashMap;
use std::fmt;

use crate::model::AppContext;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A resolved registry URL with optional authentication.
///
/// The `url_template` contains a `{}` placeholder for the package name.
/// Example: `"https://pypi.org/pypi/{}/json"` or `"https://private.repo/simple/{}/"`.
#[derive(Clone)]
pub(crate) struct RegistryEndpoint {
    /// URL template with `{}` for the package name.
    pub(crate) url_template: String,

    /// Optional auth to attach to requests for this endpoint.
    pub(crate) auth: Option<RegistryAuth>,

    /// Human-readable label for warning messages.
    /// MUST NOT contain auth tokens or credentials.
    pub(crate) label: String,

    /// HTTP method to use for existence check.
    pub(crate) method: HttpMethod,
}

impl RegistryEndpoint {
    /// Build the check URL for a given package name.
    pub(crate) fn url_for(&self, package: &str) -> String {
        self.url_template.replace("{}", package)
    }
}

impl fmt::Debug for RegistryEndpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RegistryEndpoint")
            .field("url_template", &self.url_template)
            .field("auth", &self.auth)
            .field("label", &self.label)
            .field("method", &self.method)
            .finish()
    }
}

/// Authentication method for a registry endpoint.
#[derive(Clone)]
pub(crate) enum RegistryAuth {
    /// Bearer token (npm `_authToken`, some private PyPI registries).
    Bearer(String),

    /// Basic auth (username:password, typically from URL-embedded credentials or netrc).
    Basic { username: String, password: String },
}

impl RegistryAuth {
    /// Returns the value of the `Authorization` header for this auth method.
    pub(crate) fn header_value(&self) -> String {
        match self {
            RegistryAuth::Bearer(token) => format!("Bearer {}", token),
            RegistryAuth::Basic { username, password } => {
                format!(
                    "Basic {}",
                    base64_encode(&format!("{}:{}", username, password))
                )
            }
        }
    }
}

impl fmt::Debug for RegistryAuth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RegistryAuth::Bearer(_) => f.debug_tuple("Bearer").field(&"[REDACTED]").finish(),
            RegistryAuth::Basic { username, .. } => f
                .debug_struct("Basic")
                .field("username", username)
                .field("password", &"[REDACTED]")
                .finish(),
        }
    }
}

/// HTTP method to use for an existence check request.
#[derive(Debug, Clone, Copy)]
pub(crate) enum HttpMethod {
    Head,
    Get,
}

/// Resolved npm registry configuration.
#[derive(Debug)]
pub(crate) struct NpmRegistryConfig {
    /// Default registries to check for unscoped packages.
    pub(crate) defaults: Vec<RegistryEndpoint>,
    /// Scope-specific registries. Key is the scope including `@` prefix.
    pub(crate) scopes: HashMap<String, Vec<RegistryEndpoint>>,
}

impl NpmRegistryConfig {
    /// Get the endpoints to check for a given package name.
    pub(crate) fn endpoints_for(&self, package: &str) -> &[RegistryEndpoint] {
        if let Some(scope) = package.split('/').next().filter(|s| s.starts_with('@'))
            && let Some(scoped) = self.scopes.get(scope)
        {
            return scoped;
        }
        &self.defaults
    }
}

// ---------------------------------------------------------------------------
// Public resolution functions
// ---------------------------------------------------------------------------

/// Resolve PyPI registry endpoints from environment and config files.
///
/// Resolution order:
/// 1. `PIP_INDEX_URL` env var (replaces default pypi.org entirely)
/// 2. `PIP_EXTRA_INDEX_URL` env var (comma or space separated)
/// 3. `UV_INDEX_URL` env var
/// 4. `pip.conf` / `pip.ini` -- `[global] index-url` and `extra-index-url`
/// 5. `pyproject.toml` -- `[tool.uv.index]` entries
/// 6. Default: `https://pypi.org/pypi/{}/json` (ONLY if no primary index was set)
///
/// When `PIP_INDEX_URL` is set, pypi.org is NOT added as a fallback.
/// Returns a non-empty Vec; falls back to public PyPI if all config fails.
pub(crate) fn resolve_pypi(ctx: Option<&AppContext>) -> Vec<RegistryEndpoint> {
    let mut primary: Option<RegistryEndpoint> = None;
    let mut extras: Vec<RegistryEndpoint> = Vec::new();

    // 1. PIP_INDEX_URL: replaces pypi.org as the primary index
    if let Some(url) = env_var_nonempty("PIP_INDEX_URL")
        && is_valid_http_url(&url)
    {
        let (endpoint, _) = pypi_endpoint_from_url(&url);
        primary = Some(endpoint);
    }

    // 2. PIP_EXTRA_INDEX_URL: additional endpoints (comma or space separated)
    if let Some(val) = env_var_nonempty("PIP_EXTRA_INDEX_URL") {
        for url in val
            .split([',', ' '])
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            if is_valid_http_url(url) {
                let (endpoint, _) = pypi_endpoint_from_url(url);
                extras.push(endpoint);
            }
        }
    }

    // 3. UV_INDEX_URL: additional PyPI-compatible registry
    if let Some(url) = env_var_nonempty("UV_INDEX_URL")
        && is_valid_http_url(&url)
    {
        let (endpoint, _) = pypi_endpoint_from_url(&url);
        extras.push(endpoint);
    }

    // 4. pip.conf / pip.ini
    if let Some(ctx) = ctx
        && let Some((conf_primary, conf_extras)) = read_pip_conf(ctx)
    {
        if primary.is_none() {
            primary = conf_primary;
        }
        extras.extend(conf_extras);
    }

    // 5. pyproject.toml [[tool.uv.index]] entries
    if let Some(ctx) = ctx {
        let uv_extras = read_uv_indexes(ctx);
        extras.extend(uv_extras);
    }

    // 6. Default fallback: pypi.org (ONLY when no primary configured from any source)
    let mut all = Vec::new();
    if let Some(ep) = primary {
        all.push(ep);
    } else {
        all.push(default_pypi_endpoint());
    }
    all.extend(extras);

    // Deduplicate by url_template (case-insensitive, preserve first occurrence)
    deduplicate(all)
}

/// Resolve npm registry configuration from environment and config files.
///
/// Resolution order:
/// 1. `NPM_CONFIG_REGISTRY` env var
/// 2. `.npmrc` in `ctx.cwd` (project-level)
/// 3. `~/.npmrc` (user-level)
/// 4. Default: `https://registry.npmjs.org/{}`
pub(crate) fn resolve_npm(ctx: Option<&AppContext>) -> NpmRegistryConfig {
    let mut defaults: Vec<RegistryEndpoint> = Vec::new();
    let mut scopes: HashMap<String, Vec<RegistryEndpoint>> = HashMap::new();

    // 1. NPM_CONFIG_REGISTRY: replaces the default registry
    if let Some(url) = env_var_nonempty("NPM_CONFIG_REGISTRY")
        && is_valid_http_url(&url)
    {
        let ep = npm_endpoint_from_url(&url, None);
        defaults.push(ep);
    }

    // 2 & 3. .npmrc files: project-level then user-level
    if let Some(ctx) = ctx {
        // Project-level
        let project_npmrc = ctx.cwd.join(".npmrc");
        if let Ok(content) = std::fs::read_to_string(&project_npmrc) {
            let parsed = parse_npmrc(&content);
            let allow_override = defaults.is_empty();
            merge_npmrc_into(&mut defaults, &mut scopes, parsed, allow_override);
        }

        // User-level
        if let Some(home) = &ctx.home_dir {
            let user_npmrc = home.join(".npmrc");
            if let Ok(content) = std::fs::read_to_string(&user_npmrc) {
                let parsed = parse_npmrc(&content);
                let allow_override = defaults.is_empty();
                merge_npmrc_into(&mut defaults, &mut scopes, parsed, allow_override);
            }
        }
    }

    // 4. Default fallback
    if defaults.is_empty() {
        defaults.push(default_npm_endpoint());
    }

    defaults = deduplicate(defaults);

    NpmRegistryConfig { defaults, scopes }
}

// ---------------------------------------------------------------------------
// Config file readers (read file, call parsers)
// ---------------------------------------------------------------------------

/// Read pip.conf and return (primary, extras) endpoints.
/// Returns None if no config file found or parsing fails.
fn read_pip_conf(ctx: &AppContext) -> Option<(Option<RegistryEndpoint>, Vec<RegistryEndpoint>)> {
    let content = read_pip_conf_file(ctx)?;
    let result = parse_pip_conf(&content);
    Some(result)
}

/// Locate and read the pip config file. Returns None if not found.
fn read_pip_conf_file(ctx: &AppContext) -> Option<String> {
    // 1. $PIP_CONFIG_FILE
    if let Some(path) = env_var_nonempty("PIP_CONFIG_FILE")
        && let Ok(content) = std::fs::read_to_string(&path)
    {
        return Some(content);
    }

    // 2. $XDG_CONFIG_HOME/pip/pip.conf
    if let Some(xdg) = env_var_nonempty("XDG_CONFIG_HOME") {
        let path = std::path::Path::new(&xdg).join("pip").join("pip.conf");
        if let Ok(content) = std::fs::read_to_string(&path) {
            return Some(content);
        }
    }

    // On Windows, check $APPDATA/pip/pip.ini
    #[cfg(windows)]
    if let Some(appdata) = env_var_nonempty("APPDATA") {
        let path = std::path::Path::new(&appdata).join("pip").join("pip.ini");
        if let Ok(content) = std::fs::read_to_string(&path) {
            return Some(content);
        }
    }

    if let Some(home) = &ctx.home_dir {
        // 3. ~/.config/pip/pip.conf
        let path = home.join(".config").join("pip").join("pip.conf");
        if let Ok(content) = std::fs::read_to_string(&path) {
            return Some(content);
        }

        // 4. ~/.pip/pip.conf (legacy)
        let path = home.join(".pip").join("pip.conf");
        if let Ok(content) = std::fs::read_to_string(&path) {
            return Some(content);
        }
    }

    None
}

/// Read pyproject.toml and extract [[tool.uv.index]] entries.
fn read_uv_indexes(ctx: &AppContext) -> Vec<RegistryEndpoint> {
    let path = ctx.cwd.join("pyproject.toml");
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    let urls = extract_uv_indexes(&content);
    urls.into_iter()
        .filter(|url| is_valid_http_url(url))
        .map(|url| {
            let (ep, _) = pypi_endpoint_from_url(&url);
            ep
        })
        .collect()
}

/// Merge a parsed npmrc result into defaults and scopes maps.
/// `allow_default_override`: only update the default registry if no default is yet set.
fn merge_npmrc_into(
    defaults: &mut Vec<RegistryEndpoint>,
    scopes: &mut HashMap<String, Vec<RegistryEndpoint>>,
    parsed: NpmrcParsed,
    allow_default_override: bool,
) {
    if allow_default_override && let Some(ep) = parsed.default_registry {
        defaults.push(ep);
    }
    for (scope, ep) in parsed.scoped_registries {
        scopes.entry(scope).or_default().push(ep);
    }
}

// ---------------------------------------------------------------------------
// Parsers (take &str content, no file I/O)
// ---------------------------------------------------------------------------

/// Parsed result from an .npmrc file.
struct NpmrcParsed {
    default_registry: Option<RegistryEndpoint>,
    scoped_registries: Vec<(String, RegistryEndpoint)>,
}

/// Parse .npmrc content and return registry endpoints.
///
/// Handles:
/// - `registry=URL`
/// - `//HOST/:_authToken=TOKEN` or `//HOST:_authToken=TOKEN`
/// - `@SCOPE:registry=URL`
///
/// Fails gracefully: malformed lines are skipped.
fn parse_npmrc(content: &str) -> NpmrcParsed {
    // Collect entries first, then match auth tokens to registries by hostname.
    let mut default_url: Option<String> = None;
    let mut auth_tokens: HashMap<String, String> = HashMap::new(); // host -> token
    let mut scope_entries: Vec<(String, String)> = Vec::new(); // scope -> url

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }

        let Some(eq_pos) = line.find('=') else {
            continue;
        };
        let key = line[..eq_pos].trim();
        let value = line[eq_pos + 1..].trim();

        if key == "registry" {
            if is_valid_http_url(value) {
                default_url = Some(value.to_string());
            }
        } else if let Some(host_part) = key.strip_prefix("//") {
            // Auth token line: `//HOST/:_authToken=TOKEN` or `//HOST:_authToken=TOKEN`
            let host = host_part
                .trim_end_matches("/:_authToken")
                .trim_end_matches(":_authToken")
                .trim_end_matches('/');
            // Also handle `//host/_authToken` (without colon)
            let host = if host.ends_with("/_authToken") {
                host.trim_end_matches("/_authToken")
            } else {
                host
            };
            if !host.is_empty() && !value.is_empty() {
                auth_tokens.insert(host.to_string(), value.to_string());
            }
        } else if let Some(scope) = key.strip_suffix(":registry") {
            // Scoped registry: `@SCOPE:registry=URL`
            if scope.starts_with('@') && is_valid_http_url(value) {
                scope_entries.push((scope.to_string(), value.to_string()));
            }
        }
    }

    let default_registry = default_url.map(|url| {
        let host = extract_host(&url);
        let auth = host
            .and_then(|h| auth_tokens.get(h))
            .map(|token| RegistryAuth::Bearer(token.clone()));
        npm_endpoint_from_url(&url, auth)
    });

    let scoped_registries = scope_entries
        .into_iter()
        .map(|(scope, url)| {
            let host = extract_host(&url);
            let auth = host
                .and_then(|h| auth_tokens.get(h))
                .map(|token| RegistryAuth::Bearer(token.clone()));
            let ep = npm_endpoint_from_url(&url, auth);
            (scope, ep)
        })
        .collect();

    NpmrcParsed {
        default_registry,
        scoped_registries,
    }
}

/// Parse pip.conf / pip.ini content.
///
/// Returns (primary, extras) where primary is the `index-url` and extras are `extra-index-url`.
/// Fails open: any parse error returns empty results.
pub(crate) fn parse_pip_conf(content: &str) -> (Option<RegistryEndpoint>, Vec<RegistryEndpoint>) {
    let mut in_global = false;
    let mut index_url: Option<String> = None;
    let mut extra_urls: Vec<String> = Vec::new();
    // True while consuming indented continuation lines of extra-index-url.
    let mut collecting_extra = false;

    for line in content.lines() {
        let trimmed = line.trim();

        if trimmed.starts_with('[') {
            collecting_extra = false;
            in_global = trimmed.eq_ignore_ascii_case("[global]");
            continue;
        }

        if !in_global {
            continue;
        }

        if collecting_extra && (line.starts_with(' ') || line.starts_with('\t')) {
            let url = trimmed.to_string();
            if !url.is_empty() && is_valid_http_url(&url) {
                extra_urls.push(url);
            }
            continue;
        }

        collecting_extra = false;

        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with(';') {
            continue;
        }

        let sep_pos = trimmed.find(['=', ':']);
        let Some(sep_pos) = sep_pos else {
            continue;
        };
        let key = trimmed[..sep_pos].trim().to_ascii_lowercase();
        let value = trimmed[sep_pos + 1..].trim();

        match key.as_str() {
            "index-url" => {
                let url = strip_inline_comment(value);
                if is_valid_http_url(&url) {
                    index_url = Some(url);
                }
            }
            "extra-index-url" => {
                let url = strip_inline_comment(value);
                if !url.is_empty() && is_valid_http_url(&url) {
                    extra_urls.push(url);
                }
                collecting_extra = true;
            }
            _ => {}
        }
    }

    let primary = index_url.map(|url| {
        let (ep, _) = pypi_endpoint_from_url(&url);
        ep
    });
    let extras = extra_urls
        .into_iter()
        .filter(|url| is_valid_http_url(url))
        .map(|url| {
            let (ep, _) = pypi_endpoint_from_url(&url);
            ep
        })
        .collect();

    (primary, extras)
}

/// Extract [[tool.uv.index]] URL entries from pyproject.toml content.
///
/// Returns a list of URL strings (not endpoints; caller converts).
/// Skips entries with `default = true`.
/// Fails open on any parse error.
pub(crate) fn extract_uv_indexes(content: &str) -> Vec<String> {
    let mut urls = Vec::new();
    let mut in_uv_index = false;
    let mut current_url: Option<String> = None;
    let mut current_is_default = false;

    for line in content.lines() {
        let trimmed = line.trim();

        if trimmed == "[[tool.uv.index]]" {
            // Commit any previously collected entry before starting a new section.
            if let Some(url) = current_url.take()
                && !current_is_default
            {
                urls.push(url);
            }
            current_is_default = false;
            in_uv_index = true;
            continue;
        }

        if trimmed.starts_with('[') {
            if let Some(url) = current_url.take()
                && in_uv_index
                && !current_is_default
            {
                urls.push(url);
            }
            current_is_default = false;
            in_uv_index = false;
            continue;
        }

        if !in_uv_index {
            continue;
        }

        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        let Some(eq_pos) = trimmed.find('=') else {
            continue;
        };
        let key = trimmed[..eq_pos].trim();
        let value = trimmed[eq_pos + 1..].trim();

        match key {
            "url" => {
                // Strip inline comments before stripping TOML quotes, e.g.:
                //   `"https://..." # corp mirror`  ->  `https://...`
                let value_no_comment = strip_inline_comment_after_quote(value.trim());
                let url = strip_toml_string(&value_no_comment);
                if !url.is_empty() {
                    current_url = Some(url);
                }
            }
            "default" if value.trim() == "true" => {
                current_is_default = true;
            }
            _ => {}
        }
    }

    // Commit the last entry if the file ended inside a [[tool.uv.index]] section.
    if let Some(url) = current_url
        && in_uv_index
        && !current_is_default
    {
        urls.push(url);
    }

    urls
}

// ---------------------------------------------------------------------------
// URL utilities
// ---------------------------------------------------------------------------

/// Strip embedded credentials from a URL for display.
/// `https://user:pass@host/path` -> `https://host/path`
#[cfg(test)]
fn redact_url(url: &str) -> String {
    let Some(scheme_end) = url.find("://") else {
        return url.to_string();
    };
    let after_scheme = &url[scheme_end + 3..];

    // Only look for `@` in the authority part (before the first `/`).
    let slash_pos = after_scheme.find('/').unwrap_or(after_scheme.len());
    let at_pos = after_scheme[..slash_pos].rfind('@');

    if let Some(at) = at_pos {
        let scheme = &url[..scheme_end + 3];
        let rest = &after_scheme[at + 1..];
        format!("{}{}", scheme, rest)
    } else {
        url.to_string()
    }
}

/// Extract embedded credentials from a URL if present.
/// Returns `(cleaned_url, Option<RegistryAuth>)`.
fn extract_credentials(url: &str) -> (String, Option<RegistryAuth>) {
    let Some(scheme_end) = url.find("://") else {
        return (url.to_string(), None);
    };
    let after_scheme = &url[scheme_end + 3..];
    let slash_pos = after_scheme.find('/').unwrap_or(after_scheme.len());
    let at_pos = after_scheme[..slash_pos].rfind('@');

    if let Some(at) = at_pos {
        let credentials = &after_scheme[..at];
        let scheme = &url[..scheme_end + 3];
        let rest = &after_scheme[at + 1..];
        let clean_url = format!("{}{}", scheme, rest);

        let auth = if let Some(colon) = credentials.find(':') {
            let username = credentials[..colon].to_string();
            let password = credentials[colon + 1..].to_string();
            Some(RegistryAuth::Basic { username, password })
        } else {
            // Token-style: `token@host` with no colon — treat as bearer.
            Some(RegistryAuth::Bearer(credentials.to_string()))
        };

        (clean_url, auth)
    } else {
        (url.to_string(), None)
    }
}

/// Build a PyPI endpoint from a URL.
///
/// For pypi.org, uses the JSON API (`/pypi/{}/json`) with HEAD.
/// For all other URLs, uses the Simple API format (`{base}/{}/`) with GET.
///
/// Returns `(endpoint, extracted_auth)`.
fn pypi_endpoint_from_url(url: &str) -> (RegistryEndpoint, Option<RegistryAuth>) {
    let (clean_url, auth) = extract_credentials(url);
    let label = pypi_label_from_url(&clean_url);

    let (url_template, method) = if is_pypi_org(&clean_url) {
        let template = "https://pypi.org/pypi/{}/json".to_string();
        (template, HttpMethod::Head)
    } else {
        let base = normalize_trailing_slash(&clean_url);
        let template = format!("{}{{}}/", base);
        (template, HttpMethod::Get)
    };

    let endpoint = RegistryEndpoint {
        url_template,
        auth: auth.clone(),
        label,
        method,
    };
    (endpoint, auth)
}

/// Build an npm endpoint from a URL with optional auth.
fn npm_endpoint_from_url(url: &str, auth: Option<RegistryAuth>) -> RegistryEndpoint {
    let (clean_url, embedded_auth) = extract_credentials(url);
    // Explicit auth (from .npmrc _authToken) takes priority over URL-embedded credentials.
    let resolved_auth = auth.or(embedded_auth);
    let label = npm_label_from_url(&clean_url);
    let base = normalize_trailing_slash(&clean_url);
    let url_template = format!("{}{{}}", base);

    RegistryEndpoint {
        url_template,
        auth: resolved_auth,
        label,
        method: HttpMethod::Head,
    }
}

/// Returns the default public PyPI endpoint.
fn default_pypi_endpoint() -> RegistryEndpoint {
    RegistryEndpoint {
        url_template: "https://pypi.org/pypi/{}/json".to_string(),
        auth: None,
        label: "PyPI".to_string(),
        method: HttpMethod::Head,
    }
}

/// Returns the default public npm endpoint.
fn default_npm_endpoint() -> RegistryEndpoint {
    RegistryEndpoint {
        url_template: "https://registry.npmjs.org/{}".to_string(),
        auth: None,
        label: "npm".to_string(),
        method: HttpMethod::Head,
    }
}

/// Build a human-readable label for a PyPI registry URL.
fn pypi_label_from_url(url: &str) -> String {
    if is_pypi_org(url) {
        "PyPI".to_string()
    } else if let Some(host) = extract_host(url) {
        format!("private ({})", host)
    } else {
        "private registry".to_string()
    }
}

/// Build a human-readable label for an npm registry URL.
fn npm_label_from_url(url: &str) -> String {
    if is_npmjs_org(url) {
        "npm".to_string()
    } else if let Some(host) = extract_host(url) {
        format!("private ({})", host)
    } else {
        "private registry".to_string()
    }
}

/// Check if a URL points to the public pypi.org.
fn is_pypi_org(url: &str) -> bool {
    extract_host(url).map(|h| h == "pypi.org").unwrap_or(false)
}

/// Check if a URL points to the public registry.npmjs.org.
fn is_npmjs_org(url: &str) -> bool {
    extract_host(url)
        .map(|h| h == "registry.npmjs.org")
        .unwrap_or(false)
}

/// Extract the hostname from a URL (without port).
fn extract_host(url: &str) -> Option<&str> {
    let after_scheme = url.find("://").map(|i| &url[i + 3..])?;
    let after_at = if let Some(slash) = after_scheme.find('/')
        && let Some(at) = after_scheme[..slash].rfind('@')
    {
        &after_scheme[at + 1..]
    } else if let Some(at) = after_scheme.rfind('@') {
        &after_scheme[at + 1..]
    } else {
        after_scheme
    };
    let host_end = after_at
        .find(['/', '?', '#', ':'])
        .unwrap_or(after_at.len());
    let host = &after_at[..host_end];
    if host.is_empty() { None } else { Some(host) }
}

/// Ensure a URL ends with exactly one trailing slash.
fn normalize_trailing_slash(url: &str) -> String {
    let trimmed = url.trim_end_matches('/');
    format!("{}/", trimmed)
}

/// Check if a URL is a valid HTTP or HTTPS URL.
fn is_valid_http_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

/// Get a non-empty environment variable value.
fn env_var_nonempty(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

/// Deduplicate endpoints by URL template (case-insensitive, first wins).
fn deduplicate(endpoints: Vec<RegistryEndpoint>) -> Vec<RegistryEndpoint> {
    let mut seen = std::collections::HashSet::new();
    endpoints
        .into_iter()
        .filter(|ep| seen.insert(ep.url_template.to_ascii_lowercase()))
        .collect()
}

/// Strip an inline comment from a config value.
/// Handles: `https://example.com  # comment` -> `https://example.com`
fn strip_inline_comment(value: &str) -> String {
    let mut result = value;
    if let Some(pos) = value.find(" #").or_else(|| value.find("\t#")) {
        result = value[..pos].trim();
    }
    result.trim().to_string()
}

/// Strip inline comment that appears after a closing quote on a TOML url line.
/// `"https://example.com" # comment` -> `"https://example.com"`
fn strip_inline_comment_after_quote(value: &str) -> String {
    let trimmed = value.trim();
    if let Some(hash_pos) = trimmed.find(" #").or_else(|| trimmed.find("\t#")) {
        trimmed[..hash_pos].trim().to_string()
    } else {
        trimmed.to_string()
    }
}

/// Strip surrounding quotes from a TOML string value.
/// `"https://example.com"` -> `https://example.com`
fn strip_toml_string(value: &str) -> String {
    let trimmed = value.trim();
    let is_double_quoted = trimmed.starts_with('"') && trimmed.ends_with('"');
    let is_single_quoted = trimmed.starts_with('\'') && trimmed.ends_with('\'');
    if (is_double_quoted || is_single_quoted) && trimmed.len() >= 2 {
        trimmed[1..trimmed.len() - 1].to_string()
    } else {
        trimmed.to_string()
    }
}

/// Encode bytes as standard Base64 (RFC 4648) with `=` padding.
///
/// Used for Basic auth header construction. Input is always UTF-8 text.
/// Does NOT add a `base64` crate dependency -- this minimal implementation
/// handles the standard alphabet and short inputs (username:password).
pub(crate) fn base64_encode(input: &str) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes = input.as_bytes();
    let mut result = String::with_capacity(bytes.len().div_ceil(3) * 4);

    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };

        let combined = (b0 << 16) | (b1 << 8) | b2;

        result.push(ALPHABET[((combined >> 18) & 0x3F) as usize] as char);
        result.push(ALPHABET[((combined >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            result.push(ALPHABET[((combined >> 6) & 0x3F) as usize] as char);
        } else {
            result.push('=');
        }
        if chunk.len() > 2 {
            result.push(ALPHABET[(combined & 0x3F) as usize] as char);
        } else {
            result.push('=');
        }
    }

    result
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    // -----------------------------------------------------------------------
    // parse_npmrc tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_npmrc_basic_registry() {
        let content = "registry=https://npm.corp.com/\n";
        let parsed = parse_npmrc(content);
        let ep = parsed.default_registry.unwrap();
        assert!(ep.url_template.contains("npm.corp.com"));
    }

    #[test]
    fn test_parse_npmrc_auth_token_matched() {
        let content = "registry=https://npm.corp.com/\n//npm.corp.com/:_authToken=abc123\n";
        let parsed = parse_npmrc(content);
        let ep = parsed.default_registry.unwrap();
        assert!(matches!(ep.auth, Some(RegistryAuth::Bearer(_))));
        // Verify token is present
        if let Some(RegistryAuth::Bearer(token)) = &ep.auth {
            assert_eq!(token, "abc123");
        } else {
            panic!("expected Bearer auth");
        }
    }

    #[test]
    fn test_parse_npmrc_scoped_registry() {
        let content = "@myorg:registry=https://myorg.registry.com/\n";
        let parsed = parse_npmrc(content);
        assert!(parsed.default_registry.is_none());
        assert_eq!(parsed.scoped_registries.len(), 1);
        let (scope, ep) = &parsed.scoped_registries[0];
        assert_eq!(scope, "@myorg");
        assert!(ep.url_template.contains("myorg.registry.com"));
    }

    #[test]
    fn test_parse_npmrc_ignores_comments() {
        let content =
            "# this is a comment\n; also a comment\nregistry=https://registry.npmjs.org/\n";
        let parsed = parse_npmrc(content);
        assert!(parsed.default_registry.is_some());
    }

    #[test]
    fn test_parse_npmrc_empty_file() {
        let parsed = parse_npmrc("");
        assert!(parsed.default_registry.is_none());
        assert!(parsed.scoped_registries.is_empty());
    }

    #[test]
    fn test_parse_npmrc_malformed_lines_skipped() {
        let content = "not-a-valid-line\nregistry=https://npm.corp.com/\n";
        let parsed = parse_npmrc(content);
        // Should still parse the valid line
        assert!(parsed.default_registry.is_some());
    }

    #[test]
    fn test_parse_npmrc_url_with_trailing_slash() {
        let content = "registry=https://npm.corp.com/\n";
        let parsed = parse_npmrc(content);
        let ep = parsed.default_registry.unwrap();
        // URL template should use the URL with {} appended
        assert!(ep.url_template.ends_with("{}"));
    }

    #[test]
    fn test_parse_npmrc_url_without_trailing_slash() {
        let content = "registry=https://npm.corp.com\n";
        let parsed = parse_npmrc(content);
        let ep = parsed.default_registry.unwrap();
        // Should still be a valid template
        assert!(ep.url_template.contains("npm.corp.com"));
        assert!(ep.url_template.ends_with("{}"));
    }

    // -----------------------------------------------------------------------
    // parse_pip_conf tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_pip_conf_index_url() {
        let content = "[global]\nindex-url = https://private.repo/simple/\n";
        let (primary, extras) = parse_pip_conf(content);
        assert!(primary.is_some());
        let ep = primary.unwrap();
        assert!(ep.url_template.contains("private.repo"));
        assert!(extras.is_empty());
    }

    #[test]
    fn test_parse_pip_conf_extra_index_url_single() {
        let content = "[global]\nextra-index-url = https://extra.repo/simple/\n";
        let (primary, extras) = parse_pip_conf(content);
        assert!(primary.is_none());
        assert_eq!(extras.len(), 1);
        assert!(extras[0].url_template.contains("extra.repo"));
    }

    #[test]
    fn test_parse_pip_conf_extra_index_url_multiline() {
        let content = "[global]\nextra-index-url =\n    https://extra1.repo/simple/\n    https://extra2.repo/simple/\n";
        let (primary, extras) = parse_pip_conf(content);
        assert!(primary.is_none());
        assert_eq!(extras.len(), 2);
    }

    #[test]
    fn test_parse_pip_conf_colon_separator() {
        let content = "[global]\nindex-url: https://private.repo/simple/\n";
        let (primary, _extras) = parse_pip_conf(content);
        assert!(primary.is_some());
    }

    #[test]
    fn test_parse_pip_conf_comment_lines_ignored() {
        let content =
            "[global]\n# comment\n; also comment\nindex-url = https://private.repo/simple/\n";
        let (primary, _extras) = parse_pip_conf(content);
        assert!(primary.is_some());
    }

    #[test]
    fn test_parse_pip_conf_no_global_section() {
        let content = "[install]\nindex-url = https://private.repo/simple/\n";
        let (primary, extras) = parse_pip_conf(content);
        assert!(primary.is_none());
        assert!(extras.is_empty());
    }

    #[test]
    fn test_parse_pip_conf_case_insensitive_keys() {
        let content = "[global]\nIndex-URL = https://private.repo/simple/\n";
        let (primary, _extras) = parse_pip_conf(content);
        assert!(primary.is_some());
    }

    #[test]
    fn test_parse_pip_conf_embedded_credentials() {
        let content = "[global]\nindex-url = https://user:password@private.repo/simple/\n";
        let (primary, _extras) = parse_pip_conf(content);
        let ep = primary.unwrap();
        // URL template should NOT contain credentials
        assert!(!ep.url_template.contains("user:password"));
        // Auth should be extracted
        assert!(ep.auth.is_some());
        assert!(matches!(ep.auth, Some(RegistryAuth::Basic { .. })));
    }

    // -----------------------------------------------------------------------
    // extract_uv_indexes tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_extract_uv_indexes_basic() {
        let content = r#"[[tool.uv.index]]
url = "https://private.repo/simple/"
name = "corp-internal"
"#;
        let urls = extract_uv_indexes(content);
        assert_eq!(urls.len(), 1);
        assert_eq!(urls[0], "https://private.repo/simple/");
    }

    #[test]
    fn test_extract_uv_indexes_skips_default() {
        let content = r#"[[tool.uv.index]]
url = "https://pypi.org/simple/"
name = "pypi"
default = true
"#;
        let urls = extract_uv_indexes(content);
        assert!(urls.is_empty());
    }

    #[test]
    fn test_extract_uv_indexes_multiple() {
        let content = r#"[[tool.uv.index]]
url = "https://private1.repo/simple/"
name = "corp1"

[[tool.uv.index]]
url = "https://private2.repo/simple/"
name = "corp2"
"#;
        let urls = extract_uv_indexes(content);
        assert_eq!(urls.len(), 2);
    }

    #[test]
    fn test_extract_uv_indexes_no_sections() {
        let content = "[tool.poetry]\nname = \"myproject\"\n";
        let urls = extract_uv_indexes(content);
        assert!(urls.is_empty());
    }

    #[test]
    fn test_extract_uv_indexes_inline_comment_stripped() {
        let content = "[[tool.uv.index]]\nurl = \"https://private.repo/simple/\" # corp mirror\n";
        let urls = extract_uv_indexes(content);
        assert_eq!(urls.len(), 1);
        assert_eq!(urls[0], "https://private.repo/simple/");
    }

    #[test]
    fn test_extract_uv_indexes_malformed_ignored() {
        let content = "[[tool.uv.index]]\nurl = not-a-url\n";
        // extract_uv_indexes returns strings; the caller filters invalid URLs
        let urls = extract_uv_indexes(content);
        // just ensure it doesn't panic
        let _ = urls;
    }

    // -----------------------------------------------------------------------
    // redact_url tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_redact_url_with_user_and_password() {
        let url = "https://user:pass@host/path";
        assert_eq!(redact_url(url), "https://host/path");
    }

    #[test]
    fn test_redact_url_no_credentials() {
        let url = "https://host/path";
        assert_eq!(redact_url(url), "https://host/path");
    }

    #[test]
    fn test_redact_url_user_only_no_colon() {
        let url = "https://token@host/path";
        assert_eq!(redact_url(url), "https://host/path");
    }

    #[test]
    fn test_redact_url_empty_string() {
        assert_eq!(redact_url(""), "");
    }

    // -----------------------------------------------------------------------
    // base64_encode tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_base64_encode_empty() {
        assert_eq!(base64_encode(""), "");
    }

    #[test]
    fn test_base64_encode_man() {
        // RFC 4648: "Man" -> "TWFu"
        assert_eq!(base64_encode("Man"), "TWFu");
    }

    #[test]
    fn test_base64_encode_ma() {
        // "Ma" -> "TWE="
        assert_eq!(base64_encode("Ma"), "TWE=");
    }

    #[test]
    fn test_base64_encode_m() {
        // "M" -> "TQ=="
        assert_eq!(base64_encode("M"), "TQ==");
    }

    #[test]
    fn test_base64_encode_username_password() {
        // Standard use case: username:password
        let encoded = base64_encode("user:secret");
        assert_eq!(encoded, "dXNlcjpzZWNyZXQ=");
    }

    #[test]
    fn test_base64_encode_padding() {
        // All three padding cases covered: len % 3 == 0, 1, 2
        assert!(base64_encode("abc").len() == 4);
        assert!(base64_encode("ab").ends_with('='));
        assert!(base64_encode("a").ends_with("=="));
    }

    // -----------------------------------------------------------------------
    // RegistryEndpoint::url_for tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_url_for_simple() {
        let ep = RegistryEndpoint {
            url_template: "https://pypi.org/pypi/{}/json".to_string(),
            auth: None,
            label: "PyPI".to_string(),
            method: HttpMethod::Head,
        };
        assert_eq!(
            ep.url_for("requests"),
            "https://pypi.org/pypi/requests/json"
        );
    }

    #[test]
    fn test_url_for_scoped_npm() {
        let ep = RegistryEndpoint {
            url_template: "https://registry.npmjs.org/{}".to_string(),
            auth: None,
            label: "npm".to_string(),
            method: HttpMethod::Head,
        };
        // Scoped package: @ and / are passed through without encoding
        assert_eq!(
            ep.url_for("@babel/core"),
            "https://registry.npmjs.org/@babel/core"
        );
    }

    // -----------------------------------------------------------------------
    // NpmRegistryConfig::endpoints_for tests
    // -----------------------------------------------------------------------

    fn make_endpoint(url_template: &str, label: &str) -> RegistryEndpoint {
        RegistryEndpoint {
            url_template: url_template.to_string(),
            auth: None,
            label: label.to_string(),
            method: HttpMethod::Head,
        }
    }

    #[test]
    fn test_endpoints_for_unscoped() {
        let config = NpmRegistryConfig {
            defaults: vec![make_endpoint("https://registry.npmjs.org/{}", "npm")],
            scopes: HashMap::new(),
        };
        let eps = config.endpoints_for("lodash");
        assert_eq!(eps.len(), 1);
        assert!(eps[0].url_template.contains("npmjs.org"));
    }

    #[test]
    fn test_endpoints_for_scoped_configured() {
        let mut scopes = HashMap::new();
        scopes.insert(
            "@myorg".to_string(),
            vec![make_endpoint("https://myorg.registry.com/{}", "myorg")],
        );
        let config = NpmRegistryConfig {
            defaults: vec![make_endpoint("https://registry.npmjs.org/{}", "npm")],
            scopes,
        };
        let eps = config.endpoints_for("@myorg/mypackage");
        assert_eq!(eps.len(), 1);
        assert!(eps[0].url_template.contains("myorg.registry.com"));
    }

    #[test]
    fn test_endpoints_for_scoped_unconfigured() {
        let config = NpmRegistryConfig {
            defaults: vec![make_endpoint("https://registry.npmjs.org/{}", "npm")],
            scopes: HashMap::new(),
        };
        // Scoped package with no scope config -> falls through to defaults
        let eps = config.endpoints_for("@unknown/pkg");
        assert_eq!(eps.len(), 1);
        assert!(eps[0].url_template.contains("npmjs.org"));
    }

    // -----------------------------------------------------------------------
    // resolve_pypi tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_resolve_pypi_default_only() {
        // Test that the default endpoint format is correct
        let ep = default_pypi_endpoint();
        assert_eq!(ep.url_template, "https://pypi.org/pypi/{}/json");
        assert_eq!(ep.label, "PyPI");
        assert!(matches!(ep.method, HttpMethod::Head));
    }

    #[test]
    fn test_resolve_npm_default_only() {
        let ep = default_npm_endpoint();
        assert_eq!(ep.url_template, "https://registry.npmjs.org/{}");
        assert_eq!(ep.label, "npm");
        assert!(matches!(ep.method, HttpMethod::Head));
    }

    // -----------------------------------------------------------------------
    // RegistryAuth Debug redaction tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_registry_auth_bearer_debug_redacts_secret() {
        let auth = RegistryAuth::Bearer("super-secret-token".to_string());
        let debug_str = format!("{:?}", auth);
        assert!(debug_str.contains("[REDACTED]"));
        assert!(!debug_str.contains("super-secret-token"));
    }

    #[test]
    fn test_registry_auth_basic_debug_shows_username_redacts_password() {
        let auth = RegistryAuth::Basic {
            username: "alice".to_string(),
            password: "secret-password".to_string(),
        };
        let debug_str = format!("{:?}", auth);
        assert!(debug_str.contains("[REDACTED]"));
        assert!(debug_str.contains("alice"));
        assert!(!debug_str.contains("secret-password"));
    }

    // -----------------------------------------------------------------------
    // RegistryAuth::header_value tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_header_value_bearer() {
        let auth = RegistryAuth::Bearer("my-token".to_string());
        assert_eq!(auth.header_value(), "Bearer my-token");
    }

    #[test]
    fn test_header_value_basic() {
        let auth = RegistryAuth::Basic {
            username: "user".to_string(),
            password: "password".to_string(),
        };
        // "user:password" -> dXNlcjpwYXNzd29yZA==
        assert_eq!(
            auth.header_value(),
            format!("Basic {}", base64_encode("user:password"))
        );
    }

    // -----------------------------------------------------------------------
    // Deduplication tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_deduplication_same_url_template() {
        let ep1 = make_endpoint("https://pypi.org/pypi/{}/json", "PyPI");
        let ep2 = make_endpoint("https://pypi.org/pypi/{}/json", "PyPI-dup");
        let deduped = deduplicate(vec![ep1, ep2]);
        assert_eq!(deduped.len(), 1);
        // First occurrence preserved
        assert_eq!(deduped[0].label, "PyPI");
    }

    #[test]
    fn test_deduplication_different_paths_not_deduped() {
        let ep1 = make_endpoint(
            "https://nexus.corp.com/repository/pypi-internal/simple/{}/",
            "internal",
        );
        let ep2 = make_endpoint(
            "https://nexus.corp.com/repository/pypi-external/simple/{}/",
            "external",
        );
        let deduped = deduplicate(vec![ep1, ep2]);
        assert_eq!(deduped.len(), 2);
    }

    #[test]
    fn test_deduplication_case_insensitive() {
        let ep1 = make_endpoint("https://PyPI.org/pypi/{}/json", "PyPI");
        let ep2 = make_endpoint("https://pypi.org/pypi/{}/json", "PyPI-lower");
        let deduped = deduplicate(vec![ep1, ep2]);
        assert_eq!(deduped.len(), 1);
    }

    // -----------------------------------------------------------------------
    // PIP_INDEX_URL override tests (using env-based resolution helpers)
    // -----------------------------------------------------------------------

    #[test]
    fn test_pypi_endpoint_from_url_pypi_org_uses_json_api() {
        let (ep, _) = pypi_endpoint_from_url("https://pypi.org/simple/");
        // Even if the URL points to pypi.org/simple, we use the JSON API
        assert_eq!(ep.url_template, "https://pypi.org/pypi/{}/json");
        assert!(matches!(ep.method, HttpMethod::Head));
    }

    #[test]
    fn test_pypi_endpoint_from_url_private_uses_simple_api() {
        let (ep, _) = pypi_endpoint_from_url("https://private.repo/simple/");
        assert!(ep.url_template.contains("private.repo"));
        assert!(ep.url_template.ends_with("{}/"));
        assert!(matches!(ep.method, HttpMethod::Get));
    }

    #[test]
    fn test_pypi_endpoint_from_url_private_no_trailing_slash() {
        let (ep, _) = pypi_endpoint_from_url("https://private.repo/simple");
        // Should be normalized to add trailing slash before {}
        assert!(ep.url_template.ends_with("{}/"));
    }

    // -----------------------------------------------------------------------
    // base64_encode RFC 4648 test vectors
    // -----------------------------------------------------------------------

    #[test]
    fn test_base64_encode_rfc4648_vectors() {
        // From RFC 4648 Section 10
        assert_eq!(base64_encode(""), "");
        assert_eq!(base64_encode("f"), "Zg==");
        assert_eq!(base64_encode("fo"), "Zm8=");
        assert_eq!(base64_encode("foo"), "Zm9v");
        assert_eq!(base64_encode("foob"), "Zm9vYg==");
        assert_eq!(base64_encode("fooba"), "Zm9vYmE=");
        assert_eq!(base64_encode("foobar"), "Zm9vYmFy");
    }

    // -----------------------------------------------------------------------
    // extract_host tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_extract_host_basic() {
        assert_eq!(
            extract_host("https://example.com/path"),
            Some("example.com")
        );
    }

    #[test]
    fn test_extract_host_with_port() {
        // Port stripped
        assert_eq!(
            extract_host("https://example.com:8080/path"),
            Some("example.com")
        );
    }

    #[test]
    fn test_extract_host_no_path() {
        assert_eq!(extract_host("https://example.com"), Some("example.com"));
    }

    // -----------------------------------------------------------------------
    // Edge case: empty/invalid config values
    // -----------------------------------------------------------------------

    #[test]
    fn test_is_valid_http_url_http() {
        assert!(is_valid_http_url("http://example.com"));
    }

    #[test]
    fn test_is_valid_http_url_https() {
        assert!(is_valid_http_url("https://example.com"));
    }

    #[test]
    fn test_is_valid_http_url_file_scheme_rejected() {
        assert!(!is_valid_http_url("file:///local/path"));
    }

    #[test]
    fn test_is_valid_http_url_ftp_rejected() {
        assert!(!is_valid_http_url("ftp://example.com"));
    }

    #[test]
    fn test_is_valid_http_url_empty_rejected() {
        assert!(!is_valid_http_url(""));
    }
}
