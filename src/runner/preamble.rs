/// Generate the preamble for a given language.
///
/// Returns `None` for languages that don't have a preamble (llm, ruby,
/// unknown languages). The preamble defines `creft_print`, `creft_status`,
/// and `creft_prompt` as callable functions in the target language.
///
/// The preamble assumes fd 3 (write, block → creft) and fd 4 (read,
/// creft → block) are open. If they aren't (non-unix fallback, or something
/// unexpected), the functions silently fail.
///
/// The Node preamble is compatible with both CommonJS and ESM. In ESM
/// contexts (e.g., blocks that use top-level `await`), the channel functions
/// become no-ops because `require` is not available — but the block runs
/// without a `ERR_AMBIGUOUS_MODULE_SYNTAX` error.
pub(crate) fn for_language(lang: &str) -> Option<String> {
    match lang {
        "bash" | "sh" | "zsh" => Some(BASH_PREAMBLE.to_string()),
        "python" | "python3" => Some(PYTHON_PREAMBLE.to_string()),
        "node" | "javascript" | "js" => Some(NODE_PREAMBLE.to_string()),
        _ => None,
    }
}

/// Bash preamble defining creft_print, creft_status, and creft_prompt.
///
/// Uses pure bash parameter expansion for JSON escaping — no subprocess
/// forks per call. Writes silently fail if fd 3 is not open (2>/dev/null).
const BASH_PREAMBLE: &str = r#"# -- creft runtime bindings --
_creft_escape() {
  local s="$1"
  s="${s//\\/\\\\}"
  s="${s//\"/\\\"}"
  s="${s//$'\t'/\\t}"
  s="${s//$'\n'/\\n}"
  s="${s//$'\r'/\\r}"
  printf '%s' "$s"
}
creft_print() {
  printf '{"type":"print","message":"%s"}\n' "$(_creft_escape "$1")" >&3 2>/dev/null
}
creft_status() {
  printf '{"type":"status","message":"%s"}\n' "$(_creft_escape "$1")" >&3 2>/dev/null
}
creft_prompt() {
  local _creft_id
  _creft_id="prompt_$$_$RANDOM"
  local _creft_choices=""
  if [ -n "$2" ]; then
    _creft_choices="$(_creft_escape "$2")"
  fi
  printf '{"type":"prompt","id":"%s","question":"%s","choices":"%s"}\n' \
    "$_creft_id" \
    "$(_creft_escape "$1")" \
    "$_creft_choices" >&3 2>/dev/null
  local _creft_response
  read -r _creft_response <&4
  printf '%s' "$_creft_response" | sed 's/.*"value":"\([^"]*\)".*/\1/'
}
# -- end creft runtime bindings --
"#;

/// Python preamble defining creft_print, creft_status, and creft_prompt.
///
/// Lazily initializes file objects for fd 3 and fd 4 on first use with
/// closefd=False so Python's GC doesn't close the underlying file descriptors.
/// OSError (e.g., fd not open on non-unix) is silently swallowed.
const PYTHON_PREAMBLE: &str = r#"# -- creft runtime bindings --
import json as _creft_json, os as _creft_os, random as _creft_random
_creft_fd3 = None
_creft_fd4 = None
def _creft_write(obj):
    global _creft_fd3
    try:
        if _creft_fd3 is None:
            _creft_fd3 = _creft_os.fdopen(3, 'w', closefd=False)
        _creft_fd3.write(_creft_json.dumps(obj) + '\n')
        _creft_fd3.flush()
    except OSError:
        pass
def creft_print(message):
    _creft_write({"type": "print", "message": str(message)})
def creft_status(message):
    _creft_write({"type": "status", "message": str(message)})
def creft_prompt(question, choices=""):
    global _creft_fd4
    _id = f"prompt_{_creft_os.getpid()}_{_creft_random.randint(0,99999)}"
    _creft_write({"type": "prompt", "id": _id, "question": str(question), "choices": str(choices)})
    try:
        if _creft_fd4 is None:
            _creft_fd4 = _creft_os.fdopen(4, 'r', closefd=False)
        _line = _creft_fd4.readline().strip()
        return _creft_json.loads(_line).get("value", "")
    except (OSError, ValueError):
        return ""
# -- end creft runtime bindings --
"#;

/// Node preamble defining creft_print, creft_status, and creft_prompt.
///
/// Uses synchronous `fs.writeSync`/`readSync` so no async event loop setup is
/// needed. The `fs` module is loaded via a `typeof require` guard rather than a
/// bare `require()` call: this makes the preamble syntactically valid in both
/// CommonJS and ESM contexts.
///
/// In CommonJS, `require` is defined and `fs` is loaded normally. In ESM,
/// `typeof require` evaluates to `'undefined'`, the require branch is skipped,
/// and `_creft_fs` is null. With a null `_creft_fs`, all three functions become
/// no-ops — the user's ESM block with top-level `await` runs without the
/// `ERR_AMBIGUOUS_MODULE_SYNTAX` error that a bare `require()` call would trigger.
const NODE_PREAMBLE: &str = r#"// -- creft runtime bindings --
const _creft_fs = typeof require === 'function' ? require('fs') : null;
function _creft_write(obj) {
  if (!_creft_fs) return;
  try { _creft_fs.writeSync(3, JSON.stringify(obj) + '\n'); } catch(e) {}
}
function creft_print(message) { _creft_write({type:'print',message:String(message)}); }
function creft_status(message) { _creft_write({type:'status',message:String(message)}); }
function creft_prompt(question, choices) {
  if (!_creft_fs) return '';
  const id = `prompt_${process.pid}_${Math.random().toString(36).slice(2)}`;
  _creft_write({type:'prompt',id,question:String(question),choices:String(choices||'')});
  const buf = Buffer.alloc(4096);
  const n = _creft_fs.readSync(4, buf, 0, buf.length);
  try { return JSON.parse(buf.slice(0, n).toString().trim()).value || ''; } catch(e) { return ''; }
}
// -- end creft runtime bindings --
"#;

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use rstest::rstest;

    use super::for_language;

    /// Bash preamble contains all three public function definitions.
    #[test]
    fn bash_preamble_contains_all_functions() {
        let p = for_language("bash").expect("bash must have a preamble");
        assert!(
            p.contains("creft_print"),
            "bash preamble missing creft_print"
        );
        assert!(
            p.contains("creft_status"),
            "bash preamble missing creft_status"
        );
        assert!(
            p.contains("creft_prompt"),
            "bash preamble missing creft_prompt"
        );
    }

    /// Python preamble contains the def form of all three functions.
    #[test]
    fn python_preamble_contains_all_functions() {
        let p = for_language("python").expect("python must have a preamble");
        assert!(
            p.contains("def creft_print"),
            "python preamble missing def creft_print"
        );
        assert!(
            p.contains("def creft_status"),
            "python preamble missing def creft_status"
        );
        assert!(
            p.contains("def creft_prompt"),
            "python preamble missing def creft_prompt"
        );
    }

    /// Node preamble contains the function form of all three functions.
    #[test]
    fn node_preamble_contains_all_functions() {
        let p = for_language("node").expect("node must have a preamble");
        assert!(
            p.contains("function creft_print"),
            "node preamble missing function creft_print"
        );
        assert!(
            p.contains("function creft_status"),
            "node preamble missing function creft_status"
        );
        assert!(
            p.contains("function creft_prompt"),
            "node preamble missing function creft_prompt"
        );
    }

    /// Node preamble uses a typeof-guarded require rather than a bare require()
    /// call, so it is syntactically valid in ESM contexts with top-level await.
    ///
    /// A bare `require('fs')` in an ESM file triggers `ERR_AMBIGUOUS_MODULE_SYNTAX`
    /// when top-level `await` is also present. The guard keeps the preamble
    /// ESM-compatible: in ESM `typeof require` is `'undefined'`, the branch is
    /// skipped, and `_creft_fs` is null (functions no-op silently).
    #[test]
    fn node_preamble_uses_typeof_require_guard() {
        let p = for_language("node").expect("node must have a preamble");
        assert!(
            p.contains("typeof require"),
            "node preamble must use typeof require guard for ESM compatibility"
        );
        // The preamble must not contain a bare require() call at the statement
        // level. The only require() call must be inside the typeof guard.
        assert!(
            !p.contains("= require('fs')") && p.contains("? require('fs')"),
            "node preamble must gate require('fs') behind the typeof check, not call it unconditionally"
        );
    }

    /// Language aliases that are not the canonical name still resolve to a preamble.
    #[rstest]
    #[case::sh("sh")]
    #[case::zsh("zsh")]
    #[case::python3("python3")]
    #[case::javascript("javascript")]
    #[case::js("js")]
    fn alias_resolves_to_preamble(#[case] lang: &str) {
        assert!(
            for_language(lang).is_some(),
            "{lang} must resolve to a preamble"
        );
    }

    /// Languages with no preamble return None.
    #[rstest]
    #[case::llm("llm")]
    #[case::ruby("ruby")]
    #[case::cobol("cobol")]
    #[case::empty("")]
    fn unsupported_language_returns_none(#[case] lang: &str) {
        assert_eq!(for_language(lang), None, "{lang} must return None");
    }

    /// Every preamble ends with a newline so user code starts on a fresh line.
    #[test]
    fn all_preambles_end_with_newline() {
        for lang in &["bash", "python", "node"] {
            let p = for_language(lang).unwrap();
            assert!(p.ends_with('\n'), "{lang} preamble must end with newline");
        }
    }
}
