/// Generate the preamble for a given language.
///
/// Returns `None` for languages that don't have a preamble (llm, ruby,
/// unknown languages). The preamble defines `creft_print`, `creft_status`,
/// and `creft_prompt` as callable functions in the target language.
///
/// The preamble assumes fd 3 (write, block → creft) and fd 4 (read,
/// creft → block) are open. If they aren't (non-unix fallback, or something
/// unexpected), the functions silently fail or fall back to stderr.
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

/// Node (CommonJS) preamble defining creft_print, creft_status, and creft_prompt.
///
/// Uses synchronous fs.writeSync/readSync so no async event loop setup is needed.
/// ESM blocks using import/export syntax are not supported — require() fails in
/// a "type":"module" context.
const NODE_PREAMBLE: &str = r#"// -- creft runtime bindings --
const _creft_fs = require('fs');
function _creft_write(obj) {
  try { _creft_fs.writeSync(3, JSON.stringify(obj) + '\n'); } catch(e) {}
}
function creft_print(message) { _creft_write({type:'print',message:String(message)}); }
function creft_status(message) { _creft_write({type:'status',message:String(message)}); }
function creft_prompt(question, choices) {
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

    /// sh alias resolves to the bash preamble.
    #[test]
    fn sh_alias_returns_preamble() {
        let p = for_language("sh");
        assert!(p.is_some(), "sh must have a preamble");
    }

    /// zsh alias resolves to the bash preamble.
    #[test]
    fn zsh_alias_returns_preamble() {
        let p = for_language("zsh");
        assert!(p.is_some(), "zsh must have a preamble");
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

    /// python3 alias resolves to the python preamble.
    #[test]
    fn python3_alias_returns_preamble() {
        let p = for_language("python3");
        assert!(p.is_some(), "python3 must have a preamble");
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

    /// javascript and js aliases resolve to the node preamble.
    #[test]
    fn javascript_and_js_aliases_return_preamble() {
        assert!(
            for_language("javascript").is_some(),
            "javascript must have a preamble"
        );
        assert!(for_language("js").is_some(), "js must have a preamble");
    }

    /// llm blocks have no preamble — LLM runner does not execute user scripts.
    #[test]
    fn llm_returns_none() {
        assert_eq!(for_language("llm"), None);
    }

    /// ruby has no preamble in stage 2.
    #[test]
    fn ruby_returns_none() {
        assert_eq!(for_language("ruby"), None);
    }

    /// Unknown languages have no preamble.
    #[test]
    fn unknown_language_returns_none() {
        assert_eq!(for_language("cobol"), None);
        assert_eq!(for_language(""), None);
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
