/// Generate the preamble for a given language.
///
/// Returns `None` for languages that don't have a preamble (llm, unknown
/// languages). The preamble defines `creft_print`, `creft_status`,
/// `creft_prompt`, and `creft_exit` as callable functions in the target
/// language.
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
  local _progress=""
  if [ -n "$2" ]; then
    case "$2" in
      *[!0-9]*|'') ;;
      *)
        local _p="$2"
        [ "$_p" -gt 100 ] 2>/dev/null && _p=100
        _progress=",\"progress\":$_p"
        ;;
    esac
  fi
  printf '{"type":"status","message":"%s"%s}\n' "$(_creft_escape "$1")" "$_progress" >&3 2>/dev/null
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
creft_exit() {
  local _code="${1:-0}"
  printf '{"type":"exit","code":%s}\n' "$_code" >&3 2>/dev/null
  exit 0
}
creft_index() {
  local _name="$1"
  local _content="$2"
  local _global="false"
  if [ -n "$3" ]; then
    case "$3" in
      *global*true*|*global*1*) _global="true" ;;
    esac
  fi
  printf '{"type":"index","name":"%s","content":"%s","global":%s}\n' \
    "$(_creft_escape "$_name")" "$(_creft_escape "$_content")" "$_global" >&3 2>/dev/null
}
creft_search() {
  local _creft_id
  _creft_id="search_$$_$RANDOM"
  local _query="$1"
  local _name="$2"
  printf '{"type":"search","id":"%s","query":"%s","name":"%s"}\n' \
    "$_creft_id" \
    "$(_creft_escape "$_query")" \
    "$(_creft_escape "$_name")" >&3 2>/dev/null
  local _creft_response
  read -r _creft_response <&4
  case "$_creft_response" in
    *'"error"'*)
      printf 'creft_search: %s\n' "$(printf '%s' "$_creft_response" \
        | sed 's/.*"error":"\([^"]*\)".*/\1/' \
        | sed 's/\\n/\n/g')" >&2
      ;;
    *) printf '%s' "$_creft_response" \
         | sed 's/.*"results":"\([^"]*\)".*/\1/' \
         | sed 's/\\n/\n/g' ;;
  esac
}
creft_store_put() {
  local _name="$1"
  local _key="$2"
  local _value="$3"
  local _global_part=""
  if [ -n "$4" ]; then
    case "$4" in
      *global*true*|*global*1*) _global_part=',"global":true' ;;
      *global*false*|*global*0*) _global_part=',"global":false' ;;
    esac
  fi
  printf '{"type":"store_put","name":"%s","key":"%s","value":"%s"%s}\n' \
    "$(_creft_escape "$_name")" "$(_creft_escape "$_key")" \
    "$(_creft_escape "$_value")" "$_global_part" >&3 2>/dev/null
}
creft_store_get() {
  local _creft_id
  _creft_id="store_get_$$_$RANDOM"
  local _name="$1"
  local _key="$2"
  printf '{"type":"store_get","id":"%s","name":"%s","key":"%s"}\n' \
    "$_creft_id" \
    "$(_creft_escape "$_name")" \
    "$(_creft_escape "$_key")" >&3 2>/dev/null
  local _creft_response
  read -r _creft_response <&4
  case "$_creft_response" in
    *'"error"'*) ;;
    *) printf '%s' "$_creft_response" \
         | sed 's/.*"value":"\([^"]*\)".*/\1/' \
         | sed 's/\\n/\n/g' ;;
  esac
}
creft_store_search() {
  local _creft_id
  _creft_id="store_search_$$_$RANDOM"
  local _query="$1"
  local _name="$2"
  printf '{"type":"store_search","id":"%s","name":"%s","query":"%s"}\n' \
    "$_creft_id" \
    "$(_creft_escape "$_name")" \
    "$(_creft_escape "$_query")" >&3 2>/dev/null
  local _creft_response
  read -r _creft_response <&4
  case "$_creft_response" in
    *'"error"'*) ;;
    *) printf '%s' "$_creft_response" \
         | sed 's/.*"results":"\([^"]*\)".*/\1/' \
         | sed 's/\\n/\n/g' ;;
  esac
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
def creft_status(message, progress=None):
    obj = {"type": "status", "message": str(message)}
    if progress is not None:
        obj["progress"] = max(0, min(100, int(progress)))
    _creft_write(obj)
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
def creft_exit(code=0):
    _creft_write({"type": "exit", "code": int(code)})
    import sys
    sys.stdout.flush()
    sys.exit(0)
def creft_index(name, content, options=None):
    global_flag = False
    if isinstance(options, dict):
        global_flag = bool(options.get("global", False))
    _creft_write({"type": "index", "name": str(name), "content": str(content), "global": global_flag})
def creft_search(query, name):
    global _creft_fd4
    _id = f"search_{_creft_os.getpid()}_{_creft_random.randint(0,99999)}"
    _creft_write({"type": "search", "id": _id, "query": str(query), "name": str(name)})
    try:
        if _creft_fd4 is None:
            _creft_fd4 = _creft_os.fdopen(4, 'r', closefd=False)
        _line = _creft_fd4.readline().strip()
        _resp = _creft_json.loads(_line)
        if "error" in _resp:
            import sys
            print(f"creft_search: {_resp['error']}", file=sys.stderr)
            return ""
        return _resp.get("results", "").replace("\\n", "\n")
    except (OSError, ValueError):
        return ""
def creft_store_put(name, key, value, options=None):
    msg = {"type": "store_put", "name": str(name), "key": str(key),
           "value": str(value)}
    if isinstance(options, dict) and "global" in options:
        msg["global"] = bool(options["global"])
    _creft_write(msg)
def creft_store_get(name, key):
    global _creft_fd4
    _id = f"store_get_{_creft_os.getpid()}_{_creft_random.randint(0,99999)}"
    _creft_write({"type": "store_get", "id": _id, "name": str(name), "key": str(key)})
    try:
        if _creft_fd4 is None:
            _creft_fd4 = _creft_os.fdopen(4, 'r', closefd=False)
        _line = _creft_fd4.readline().strip()
        _resp = _creft_json.loads(_line)
        if "error" in _resp:
            return ""
        return _resp.get("value", "").replace("\\n", "\n")
    except (OSError, ValueError):
        return ""
def creft_store_search(query, name):
    global _creft_fd4
    _id = f"store_search_{_creft_os.getpid()}_{_creft_random.randint(0,99999)}"
    _creft_write({"type": "store_search", "id": _id, "name": str(name), "query": str(query)})
    try:
        if _creft_fd4 is None:
            _creft_fd4 = _creft_os.fdopen(4, 'r', closefd=False)
        _line = _creft_fd4.readline().strip()
        _resp = _creft_json.loads(_line)
        if "error" in _resp:
            return ""
        return _resp.get("results", "").replace("\\n", "\n")
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
function creft_status(message, progress) {
  const obj = {type:'status', message:String(message)};
  if (typeof progress === 'number') {
    obj.progress = Math.max(0, Math.min(100, Math.floor(progress)));
  }
  _creft_write(obj);
}
function creft_prompt(question, choices) {
  if (!_creft_fs) return '';
  const id = `prompt_${process.pid}_${Math.random().toString(36).slice(2)}`;
  _creft_write({type:'prompt',id,question:String(question),choices:String(choices||'')});
  const buf = Buffer.alloc(4096);
  const n = _creft_fs.readSync(4, buf, 0, buf.length);
  try { return JSON.parse(buf.slice(0, n).toString().trim()).value || ''; } catch(e) { return ''; }
}
function creft_exit(code) {
  _creft_write({type:'exit',code:typeof code === 'number' ? code : 0});
  // process.exit() terminates Node before its async stdout buffer drains,
  // silently dropping output when the write queue exceeds the OS pipe buffer.
  // Defer exit until stdout is flushed to prevent data loss.
  if (process.stdout.writableLength > 0) {
    process.stdout.once('drain', function() { process.exit(0); });
  } else {
    process.exit(0);
  }
}
function creft_index(name, content, options) {
  const global_flag = (options && typeof options.global === 'boolean') ? options.global : false;
  _creft_write({type:'index',name:String(name),content:String(content),global:global_flag});
}
function creft_search(query, name) {
  if (!_creft_fs) return '';
  const id = `search_${process.pid}_${Math.random().toString(36).slice(2)}`;
  _creft_write({type:'search',id,query:String(query),name:String(name)});
  const buf = Buffer.alloc(4096);
  const n = _creft_fs.readSync(4, buf, 0, buf.length);
  try {
    const resp = JSON.parse(buf.slice(0, n).toString().trim());
    if (resp.error) {
      try { process.stderr.write('creft_search: ' + resp.error + '\n'); } catch(e) {}
      return '';
    }
    return (resp.results || '').replace(/\\n/g, '\n');
  } catch(e) { return ''; }
}
function creft_store_put(name, key, value, options) {
  const msg = {type:'store_put',name:String(name),key:String(key),value:String(value)};
  if (options && typeof options.global === 'boolean') msg.global = options.global;
  _creft_write(msg);
}
function creft_store_get(name, key) {
  if (!_creft_fs) return '';
  const id = `store_get_${process.pid}_${Math.random().toString(36).slice(2)}`;
  _creft_write({type:'store_get',id,name:String(name),key:String(key)});
  let line = '';
  const chunk = Buffer.alloc(4096);
  for (;;) {
    const n = _creft_fs.readSync(4, chunk, 0, chunk.length);
    if (n === 0) break;
    line += chunk.slice(0, n).toString();
    if (line.indexOf('\n') !== -1) break;
  }
  try {
    const resp = JSON.parse(line.trim());
    if (resp.error) return '';
    return (resp.value || '').replace(/\\n/g, '\n');
  } catch(e) { return ''; }
}
function creft_store_search(query, name) {
  if (!_creft_fs) return '';
  const id = `store_search_${process.pid}_${Math.random().toString(36).slice(2)}`;
  _creft_write({type:'store_search',id,name:String(name),query:String(query)});
  const buf = Buffer.alloc(4096);
  const n = _creft_fs.readSync(4, buf, 0, buf.length);
  try {
    const resp = JSON.parse(buf.slice(0, n).toString().trim());
    if (resp.error) return '';
    return (resp.results || '').replace(/\\n/g, '\n');
  } catch(e) { return ''; }
}
// -- end creft runtime bindings --
"#;

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use rstest::rstest;

    use super::for_language;

    /// Bash preamble contains all public function definitions.
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
        assert!(p.contains("creft_exit"), "bash preamble missing creft_exit");
    }

    /// Python preamble contains the def form of all public functions.
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
        assert!(
            p.contains("def creft_exit"),
            "python preamble missing def creft_exit"
        );
    }

    /// Node preamble contains the function form of all public functions.
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
        assert!(
            p.contains("function creft_exit"),
            "node preamble missing function creft_exit"
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

    /// Bash preamble contains the digit-only guard that rejects non-numeric $2.
    ///
    /// The guard (`*[!0-9]*`) ensures non-numeric input falls through to spinner
    /// mode rather than producing bare unquoted text in the JSON.
    #[test]
    fn bash_preamble_has_digit_guard_for_progress() {
        let p = for_language("bash").expect("bash must have a preamble");
        assert!(
            p.contains("*[!0-9]*"),
            "bash preamble must contain digit-only guard *[!0-9]*"
        );
        assert!(
            p.contains("_progress"),
            "bash preamble must contain _progress local variable"
        );
    }

    /// Python preamble's creft_status accepts an optional progress parameter.
    #[test]
    fn python_preamble_creft_status_accepts_progress_param() {
        let p = for_language("python").expect("python must have a preamble");
        assert!(
            p.contains("def creft_status(message, progress=None)"),
            "python creft_status must accept progress=None"
        );
    }

    /// Node creft_exit defers process.exit until stdout drains rather than
    /// terminating immediately, preventing silent data loss when the write queue
    /// exceeds the OS pipe buffer.
    #[test]
    fn node_preamble_creft_exit_defers_exit_until_stdout_drains() {
        let p = for_language("node").expect("node must have a preamble");
        assert!(
            p.contains("writableLength"),
            "node creft_exit must check process.stdout.writableLength before exiting"
        );
        assert!(
            p.contains("drain"),
            "node creft_exit must defer process.exit until the drain event fires"
        );
    }

    /// Python creft_exit flushes stdout before sys.exit to prevent data loss.
    #[test]
    fn python_preamble_creft_exit_flushes_stdout() {
        let p = for_language("python").expect("python must have a preamble");
        // The flush must appear before the sys.exit call.
        let flush_pos = p
            .find("sys.stdout.flush()")
            .expect("python creft_exit must call sys.stdout.flush()");
        let exit_pos = p
            .find("sys.exit(0)")
            .expect("python creft_exit must call sys.exit(0)");
        assert!(
            flush_pos < exit_pos,
            "sys.stdout.flush() must precede sys.exit(0) in python creft_exit"
        );
    }

    /// Node preamble's creft_status accepts a second parameter.
    #[test]
    fn node_preamble_creft_status_accepts_progress_param() {
        let p = for_language("node").expect("node must have a preamble");
        assert!(
            p.contains("function creft_status(message, progress)"),
            "node creft_status must accept a second progress parameter"
        );
        assert!(
            p.contains("typeof progress === 'number'"),
            "node creft_status must guard progress with typeof check"
        );
    }

    // ── creft_index / creft_search presence ──────────────────────────────────

    /// All three preambles expose creft_index.
    #[rstest]
    #[case::bash("bash")]
    #[case::python("python")]
    #[case::node("node")]
    fn all_preambles_contain_creft_index(#[case] lang: &str) {
        let p = for_language(lang).unwrap_or_else(|| panic!("{lang} must have a preamble"));
        assert!(
            p.contains("creft_index"),
            "{lang} preamble must define creft_index"
        );
    }

    /// All three preambles expose creft_search.
    #[rstest]
    #[case::bash("bash")]
    #[case::python("python")]
    #[case::node("node")]
    fn all_preambles_contain_creft_search(#[case] lang: &str) {
        let p = for_language(lang).unwrap_or_else(|| panic!("{lang} must have a preamble"));
        assert!(
            p.contains("creft_search"),
            "{lang} preamble must define creft_search"
        );
    }

    /// Bash creft_index uses a case-based global flag detection to parse its options arg.
    #[test]
    fn bash_creft_index_parses_global_option_with_case() {
        let p = for_language("bash").expect("bash must have a preamble");
        assert!(
            p.contains("*global*true*"),
            "bash creft_index must detect global:true using case match"
        );
    }

    /// Bash creft_search generates a unique ID and reads from fd 4.
    #[test]
    fn bash_creft_search_reads_from_fd4() {
        let p = for_language("bash").expect("bash must have a preamble");
        // Uses <&4 to read the response from fd 4.
        assert!(
            p.contains("<&4"),
            "bash creft_search must read the response from fd 4 (<&4)"
        );
    }

    /// Python creft_index accepts an optional dict for options.
    #[test]
    fn python_creft_index_accepts_options_dict() {
        let p = for_language("python").expect("python must have a preamble");
        assert!(
            p.contains("def creft_index(name, content, options=None)"),
            "python creft_index must accept options=None keyword argument"
        );
    }

    /// Python creft_search reads a JSON response from fd 4.
    #[test]
    fn python_creft_search_reads_json_from_fd4() {
        let p = for_language("python").expect("python must have a preamble");
        assert!(
            p.contains("def creft_search(query, name)"),
            "python creft_search must accept query and name parameters"
        );
        assert!(
            p.contains("results"),
            "python creft_search must extract the results field from the response"
        );
    }

    /// Node creft_index defaults global to false when options is not provided.
    #[test]
    fn node_creft_index_defaults_global_to_false() {
        let p = for_language("node").expect("node must have a preamble");
        assert!(
            p.contains("function creft_index(name, content, options)"),
            "node creft_index must accept name, content, and options"
        );
        // The default must be false when options is absent.
        assert!(
            p.contains("false"),
            "node creft_index must default global_flag to false"
        );
    }

    /// Node creft_search reads synchronously from fd 4 and parses JSON.
    #[test]
    fn node_creft_search_reads_sync_from_fd4() {
        let p = for_language("node").expect("node must have a preamble");
        assert!(
            p.contains("function creft_search(query, name)"),
            "node creft_search must accept query and name"
        );
        assert!(
            p.contains("readSync(4"),
            "node creft_search must use readSync on fd 4"
        );
    }

    // ── creft_search error surfacing ──────────────────────────────────────────

    /// Bash creft_search redirects error output to stderr rather than stdout.
    #[test]
    fn bash_creft_search_error_case_writes_to_stderr() {
        let p = for_language("bash").expect("bash must have a preamble");
        assert!(
            p.contains(">&2"),
            "bash creft_search error case must redirect to stderr with >&2"
        );
    }

    /// Bash creft_search extracts the error field from the JSON response rather
    /// than printing raw JSON to stderr.
    #[test]
    fn bash_creft_search_error_case_extracts_error_field() {
        let p = for_language("bash").expect("bash must have a preamble");
        // The sed pattern that extracts the error field value.
        assert!(
            p.contains(r#""error":"\([^"]*\)""#),
            "bash creft_search must extract the error field with sed"
        );
    }

    /// Bash creft_search prefixes the error message with `creft_search:`.
    #[test]
    fn bash_creft_search_error_message_is_prefixed() {
        let p = for_language("bash").expect("bash must have a preamble");
        assert!(
            p.contains("creft_search: %s"),
            "bash creft_search error must be prefixed with 'creft_search:'"
        );
    }

    /// Python creft_search writes to stderr on error.
    #[test]
    fn python_creft_search_error_case_writes_to_stderr() {
        let p = for_language("python").expect("python must have a preamble");
        assert!(
            p.contains("sys.stderr"),
            "python creft_search error case must write to sys.stderr"
        );
    }

    /// Python creft_search prefixes the error message with `creft_search:`.
    #[test]
    fn python_creft_search_error_message_is_prefixed() {
        let p = for_language("python").expect("python must have a preamble");
        assert!(
            p.contains("creft_search:"),
            "python creft_search error must be prefixed with 'creft_search:'"
        );
    }

    /// Node creft_search writes to stderr on error.
    #[test]
    fn node_creft_search_error_case_writes_to_stderr() {
        let p = for_language("node").expect("node must have a preamble");
        assert!(
            p.contains("process.stderr"),
            "node creft_search error case must write to process.stderr"
        );
    }

    /// Node creft_search prefixes the error message with `creft_search:`.
    #[test]
    fn node_creft_search_error_message_is_prefixed() {
        let p = for_language("node").expect("node must have a preamble");
        assert!(
            p.contains("'creft_search: '"),
            "node creft_search error must be prefixed with 'creft_search:'"
        );
    }

    // ── store primitive preamble tests ────────────────────────────────────────

    /// All three preambles define creft_store_put.
    #[rstest]
    #[case::bash("bash")]
    #[case::python("python")]
    #[case::node("node")]
    fn preamble_contains_creft_store_put(#[case] lang: &str) {
        let p = for_language(lang).expect("preamble must exist");
        assert!(
            p.contains("creft_store_put"),
            "{lang} preamble missing creft_store_put"
        );
    }

    /// All three preambles define creft_store_get.
    #[rstest]
    #[case::bash("bash")]
    #[case::python("python")]
    #[case::node("node")]
    fn preamble_contains_creft_store_get(#[case] lang: &str) {
        let p = for_language(lang).expect("preamble must exist");
        assert!(
            p.contains("creft_store_get"),
            "{lang} preamble missing creft_store_get"
        );
    }

    /// All three preambles define creft_store_search.
    #[rstest]
    #[case::bash("bash")]
    #[case::python("python")]
    #[case::node("node")]
    fn preamble_contains_creft_store_search(#[case] lang: &str) {
        let p = for_language(lang).expect("preamble must exist");
        assert!(
            p.contains("creft_store_search"),
            "{lang} preamble missing creft_store_search"
        );
    }

    /// Bash creft_store_put includes global:true when the option is passed.
    #[test]
    fn bash_store_put_includes_global_true_when_option_set() {
        let p = for_language("bash").expect("bash must have a preamble");
        assert!(p.contains("global*true") || p.contains("\"global\":true"));
    }

    /// Bash creft_store_get reads from fd 4.
    #[test]
    fn bash_store_get_reads_from_fd4() {
        let p = for_language("bash").expect("bash must have a preamble");
        assert!(p.contains("read -r _creft_response <&4"));
    }

    /// Python creft_store_put omits global when options dict lacks the key.
    #[test]
    fn python_store_put_omits_global_when_not_in_options() {
        let p = for_language("python").expect("python must have a preamble");
        assert!(p.contains("\"global\" in options"));
    }

    /// Python creft_store_get reads JSON from fd 4.
    #[test]
    fn python_store_get_reads_json_from_fd4() {
        let p = for_language("python").expect("python must have a preamble");
        assert!(p.contains("store_get"));
        assert!(p.contains("_creft_fd4.readline"));
    }

    /// Node creft_store_put omits global when options not provided or global not boolean.
    #[test]
    fn node_store_put_omits_global_when_not_boolean() {
        let p = for_language("node").expect("node must have a preamble");
        assert!(p.contains("typeof options.global === 'boolean'"));
    }

    /// Node creft_store_get reads in a loop until newline (no fixed buffer limit).
    #[test]
    fn node_store_get_reads_in_loop_until_newline() {
        let p = for_language("node").expect("node must have a preamble");
        assert!(p.contains("indexOf('\\n')"));
    }

    // ── store preamble structural tests ──────────────────────────────────────

    /// Bash creft_store_put writes to fd 3 (block → creft direction).
    #[test]
    fn bash_store_put_writes_to_fd3() {
        let p = for_language("bash").expect("bash must have a preamble");
        assert!(p.contains(">&3"));
    }

    /// Bash creft_store_search reads a response from fd 4.
    #[test]
    fn bash_store_search_reads_from_fd4() {
        let p = for_language("bash").expect("bash must have a preamble");
        // creft_store_search must read the response with <&4.
        let idx = p.find("creft_store_search").expect("creft_store_search must exist");
        let after = &p[idx..];
        assert!(after.contains("<&4"));
    }

    /// Python creft_store_search reads from fd 4 and extracts results.
    #[test]
    fn python_store_search_reads_from_fd4() {
        let p = for_language("python").expect("python must have a preamble");
        assert!(p.contains("creft_store_search"));
        assert!(p.contains("_creft_fd4"));
    }

    /// Node creft_store_search uses readSync on fd 4.
    #[test]
    fn node_store_search_reads_sync_from_fd4() {
        let p = for_language("node").expect("node must have a preamble");
        let idx = p.find("creft_store_search").expect("creft_store_search must exist");
        let after = &p[idx..];
        assert!(after.contains("readSync(4"));
    }

    /// All three preambles write store_put messages with the store_put type field.
    #[rstest]
    #[case::bash("bash")]
    #[case::python("python")]
    #[case::node("node")]
    fn preamble_store_put_type_field_is_store_put(#[case] lang: &str) {
        let p = for_language(lang).expect("preamble must exist");
        assert!(p.contains("store_put"));
    }

    /// All three preambles write store_get messages with the store_get type field.
    #[rstest]
    #[case::bash("bash")]
    #[case::python("python")]
    #[case::node("node")]
    fn preamble_store_get_type_field_is_store_get(#[case] lang: &str) {
        let p = for_language(lang).expect("preamble must exist");
        assert!(p.contains("store_get"));
    }

    /// All three preambles write store_search messages with the store_search type field.
    #[rstest]
    #[case::bash("bash")]
    #[case::python("python")]
    #[case::node("node")]
    fn preamble_store_search_type_field_is_store_search(#[case] lang: &str) {
        let p = for_language(lang).expect("preamble must exist");
        assert!(p.contains("store_search"));
    }

    /// Bash creft_store_search generates a unique id with a random component.
    #[test]
    fn bash_store_search_id_has_random_component() {
        let p = for_language("bash").unwrap();
        assert!(p.contains("store_search_"));
    }

    /// Python creft_store_get extracts the value field from the JSON response.
    #[test]
    fn python_store_get_extracts_value_field() {
        let p = for_language("python").unwrap();
        assert!(p.contains("\"value\""));
    }

    /// Python creft_store_search extracts the results field.
    #[test]
    fn python_store_search_extracts_results_field() {
        let p = for_language("python").unwrap();
        assert!(p.contains("\"results\""));
    }

    /// Node creft_store_put stringifies name, key, and value.
    #[test]
    fn node_store_put_stringifies_name_key_value() {
        let p = for_language("node").unwrap();
        assert!(p.contains("String(name)"));
        assert!(p.contains("String(key)"));
        assert!(p.contains("String(value)"));
    }

    /// Node creft_store_search uses a random id for request correlation.
    #[test]
    fn node_store_search_uses_random_id() {
        let p = for_language("node").unwrap();
        assert!(p.contains("Math.random()"));
    }

    /// Bash creft_store_put uses _creft_escape for the name, key, and value.
    #[test]
    fn bash_store_put_uses_creft_escape() {
        let p = for_language("bash").unwrap();
        let idx = p.find("creft_store_put").unwrap();
        let after = &p[idx..];
        assert!(after.contains("_creft_escape"));
    }
}
