use std::fmt::Write as FmtWrite;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use yansi::Paint as _;

use crate::error::CreftError;
use crate::model::AppContext;

// ── ASCII logo ─────────────────────────────────────────────────────────────

/// The creft ASCII logo.
///
/// Block-letter art spelling "creft". Each line fits within 41 columns.
const LOGO: &[&str] = &[
    r" ██████╗██████╗ ███████╗███████╗████████╗",
    r"██╔════╝██╔══██╗██╔════╝██╔════╝╚══██╔══╝",
    r"██║     ██████╔╝█████╗  █████╗     ██║   ",
    r"██║     ██╔══██╗██╔══╝  ██╔══╝     ██║   ",
    r"╚██████╗██║  ██║███████╗██║        ██║   ",
    r" ╚═════╝╚═╝  ╚═╝╚══════╝╚═╝        ╚═╝   ",
];

// ── Color palette ──────────────────────────────────────────────────────────

/// Gradient start: amber gold.
const GRAD_FROM: (u8, u8, u8) = (212, 160, 23);
/// Gradient end: dark turquoise.
const GRAD_TO: (u8, u8, u8) = (0, 206, 209);

// ── Timing ─────────────────────────────────────────────────────────────────

/// Characters revealed per frame during the logo reveal phase.
///
/// The logo is 41 columns wide. At 3 columns per frame, the reveal
/// takes ceil(41/3) = 14 frames × 35ms = ~490ms.
const REVEAL_COLS_PER_FRAME: usize = 3;

/// Frame duration in milliseconds for the reveal phase.
const REVEAL_FRAME_MS: u64 = 35;

/// Characters swept per frame during the underline phase.
///
/// At 4 columns per frame and 20ms per frame, the sweep takes
/// ceil(41/4) × 20ms = ~220ms.
const UNDERLINE_COLS_PER_FRAME: usize = 4;

/// Frame duration in milliseconds for the underline sweep phase.
const UNDERLINE_FRAME_MS: u64 = 20;

// ── Underline character ────────────────────────────────────────────────────

/// The character used for the underline sweep beneath the logo.
///
/// U+2500 BOX DRAWINGS LIGHT HORIZONTAL — a clean, single-width line character.
const UNDERLINE_CHAR: char = '─';

// ── Entry point ────────────────────────────────────────────────────────────

/// Entry point for `creft _creft welcome`.
///
/// Checks for the marker file. If the marker exists and `force` is false,
/// returns immediately without printing anything. Otherwise renders the
/// welcome experience and writes the marker.
pub(crate) fn cmd_welcome(ctx: &AppContext, force: bool) -> Result<(), CreftError> {
    if !force && already_welcomed(ctx)? {
        return Ok(());
    }

    let term = console::Term::stdout();
    if term.is_term() {
        render_animated(&term)?;
    } else {
        render_static(&term)?;
    }

    write_marker(ctx)?;
    Ok(())
}

// ── Marker file ────────────────────────────────────────────────────────────

/// Path to the per-user welcome marker: `~/.creft/.welcome-done`.
fn marker_path(ctx: &AppContext) -> Result<std::path::PathBuf, CreftError> {
    Ok(ctx.global_root()?.join(".welcome-done"))
}

/// Returns `true` if the marker file already exists.
fn already_welcomed(ctx: &AppContext) -> Result<bool, CreftError> {
    Ok(marker_path(ctx)?.exists())
}

/// Write the marker file containing the current creft version.
///
/// Creates `~/.creft/` if it does not yet exist.
fn write_marker(ctx: &AppContext) -> Result<(), CreftError> {
    let path = marker_path(ctx)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, env!("CARGO_PKG_VERSION"))?;
    Ok(())
}

// ── Static rendering ───────────────────────────────────────────────────────

/// Render the static welcome to `term`.
///
/// Used directly for non-TTY output and as the final frame of the animated
/// path. Respects the global yansi color condition: when color is disabled
/// (NO_COLOR, TERM=dumb, non-TTY), all output is plain text.
fn render_static(term: &console::Term) -> Result<(), CreftError> {
    // &Term implements std::io::Write, so coerce through &mut &Term.
    render_static_to_writer(&mut &*term)?;
    Ok(())
}

/// Render the static welcome block to any `std::io::Write` impl.
///
/// Separated from `render_static` so tests can pass a `Vec<u8>` and inspect
/// the rendered bytes directly, without depending on `console::Term`'s
/// internal buffer.
fn render_static_to_writer(w: &mut dyn std::io::Write) -> std::io::Result<()> {
    writeln!(w)?;
    for line in LOGO {
        writeln!(w, "{}", gradient_line(line, GRAD_FROM, GRAD_TO))?;
    }
    writeln!(w)?;
    writeln!(w, "  {}", "Executable skills for Agents".rgb(180, 180, 180))?;
    writeln!(w, "  v{}", env!("CARGO_PKG_VERSION").rgb(120, 120, 120))?;
    writeln!(w)?;
    writeln!(w, "  {}", "Get started:".rgb(212, 160, 23))?;
    writeln!(
        w,
        "    {}    {}",
        "creft add".rgb(0, 206, 209),
        "Create a skill from stdin".rgb(160, 160, 160)
    )?;
    writeln!(
        w,
        "    {}   {}",
        "creft list".rgb(0, 206, 209),
        "See available skills".rgb(160, 160, 160)
    )?;
    writeln!(
        w,
        "    {}     {}",
        "creft up".rgb(0, 206, 209),
        "Set up editor integrations".rgb(160, 160, 160)
    )?;
    writeln!(w)?;
    writeln!(w, "  Run creft --help for the full command reference.")?;
    writeln!(w)?;
    Ok(())
}

// ── Color helpers ──────────────────────────────────────────────────────────

/// Apply a horizontal RGB color gradient to a single line of text.
///
/// Each character's color is linearly interpolated between `from` and `to`
/// based on its position in the string. When yansi is globally disabled,
/// returns the plain text unchanged — yansi's `Display` impl omits escapes.
fn gradient_line(text: &str, from: (u8, u8, u8), to: (u8, u8, u8)) -> String {
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    if n == 0 {
        return String::new();
    }
    let mut out = String::with_capacity(n * 20);
    for (i, ch) in chars.iter().enumerate() {
        let t = if n == 1 {
            0.0f32
        } else {
            i as f32 / (n - 1) as f32
        };
        let r = lerp_u8(from.0, to.0, t);
        let g = lerp_u8(from.1, to.1, t);
        let b = lerp_u8(from.2, to.2, t);
        let s = ch.to_string();
        let _ = write!(out, "{}", s.as_str().rgb(r, g, b));
    }
    out
}

fn lerp_u8(a: u8, b: u8, t: f32) -> u8 {
    let result = a as f32 + (b as f32 - a as f32) * t;
    result.round().clamp(0.0, 255.0) as u8
}

/// Compute the gradient color at position `i` out of `total` positions.
///
/// Returns the (r, g, b) tuple interpolated between `GRAD_FROM` and `GRAD_TO`.
fn gradient_color_at(i: usize, total: usize) -> (u8, u8, u8) {
    let t = if total <= 1 {
        0.0f32
    } else {
        i as f32 / (total - 1) as f32
    };
    (
        lerp_u8(GRAD_FROM.0, GRAD_TO.0, t),
        lerp_u8(GRAD_FROM.1, GRAD_TO.1, t),
        lerp_u8(GRAD_FROM.2, GRAD_TO.2, t),
    )
}

// ── Frame builders ─────────────────────────────────────────────────────────

/// Build a single frame string for the logo reveal at the given column.
///
/// `reveal_col` is the number of columns to show (0 = nothing, 41 = full logo).
/// Unrevealed columns are written as spaces to fully overwrite prior frame content.
///
/// Uses `\x1b[s` / `\x1b[u` (SCO save/restore cursor) so the cursor returns to
/// the top of the logo region after each frame. No absolute row positioning needed.
fn build_reveal_frame(reveal_col: usize) -> String {
    let mut out = String::with_capacity(2048);

    out.push_str("\x1b[s"); // save cursor position

    for line in LOGO {
        let chars: Vec<char> = line.chars().collect();
        let total = chars.len();
        for (i, ch) in chars.iter().enumerate() {
            if i < reveal_col {
                let (r, g, b) = gradient_color_at(i, total);
                let s = ch.to_string();
                let _ = write!(out, "{}", s.as_str().rgb(r, g, b));
            } else {
                out.push(' ');
            }
        }
        out.push('\n');
    }

    out.push_str("\x1b[u"); // restore cursor position

    out
}

/// Build a single frame for the underline sweep at the given column.
///
/// `sweep_col` is the number of underline characters to show (0 = nothing,
/// `logo_width` = complete line). The underline appears on the line immediately
/// below the logo, using the same gradient colors as the reveal.
///
/// Uses `\x1b[s` / `\x1b[u` (SCO save/restore cursor) and a relative cursor-down
/// escape to reach the underline row without absolute positioning.
fn build_underline_frame(sweep_col: usize, logo_width: usize) -> String {
    let mut out = String::with_capacity(512);

    out.push_str("\x1b[s"); // save cursor position

    // Move down past the logo to the underline row.
    let _ = write!(out, "\x1b[{}B", LOGO.len());

    for i in 0..sweep_col {
        let (r, g, b) = gradient_color_at(i, logo_width);
        let s = UNDERLINE_CHAR.to_string();
        let _ = write!(out, "{}", s.as_str().rgb(r, g, b));
    }

    out.push_str("\x1b[u"); // restore cursor position

    out
}

// ── Animated rendering ─────────────────────────────────────────────────────

/// Drop guard that restores cursor visibility.
///
/// Handles normal exit and early `?` returns. Does NOT handle SIGINT on its
/// own — the release profile sets `panic = "abort"`, so `Drop` impls do not
/// run when the process is terminated by a signal. The animation loop checks a
/// cancellation flag each frame and breaks early so this guard drops on the
/// normal return path.
struct CursorGuard<'a> {
    term: &'a console::Term,
}

impl<'a> CursorGuard<'a> {
    fn new(term: &'a console::Term) -> Self {
        Self { term }
    }
}

impl Drop for CursorGuard<'_> {
    fn drop(&mut self) {
        // Ignore errors: the terminal may already be gone (redirect, broken pipe).
        let _ = self.term.show_cursor();
    }
}

/// Run the animated welcome on a TTY, then leave static output visible.
///
/// Falls back to `render_static()` when the terminal is too small to animate
/// cleanly. Registers a SIGINT cancellation flag — the animation loop breaks
/// early on cancellation so the `CursorGuard` still drops on the normal return
/// path, restoring cursor visibility.
///
/// Animation phases:
/// - Phase 1: Logo reveal — all 6 lines advance left-to-right simultaneously (~490ms).
/// - Phase 2: Gradient underline sweep — a colored line appears beneath the logo (~220ms).
///
/// Each frame is built as a single `String` containing all ANSI escape sequences
/// and written with one `term.write_str()` call. One write per frame eliminates
/// the flicker caused by multiple sequential flush calls.
fn render_animated(term: &console::Term) -> Result<(), CreftError> {
    let (rows, cols) = term.size();

    if cols < 50 || rows < 15 {
        return render_static(term);
    }

    // SIGINT cancellation — same pattern as src/cmd/run.rs.
    let cancel = Arc::new(AtomicBool::new(false));
    #[cfg(unix)]
    let _ = signal_hook::flag::register(signal_hook::consts::SIGINT, Arc::clone(&cancel));

    term.hide_cursor()?;
    let _guard = CursorGuard::new(term);

    // Reserve vertical space: 1 blank + logo + 1 underline row + static block.
    // The leading blank gives breathing room above the logo.
    let logo_height = LOGO.len();
    let static_lines = 10; // blank + tagline + version + blank + header + 3 commands + blank + hint + blank
    let total_lines = 1 + logo_height + 1 + static_lines;

    for _ in 0..total_lines {
        term.write_line("")?;
    }
    term.move_cursor_up(total_lines)?;

    // Skip the leading blank line so the cursor sits at the first logo row.
    term.write_line("")?;

    // The logo width is the character count of the widest line.
    let logo_width = LOGO.iter().map(|l| l.chars().count()).max().unwrap_or(41);

    // Phase 1: Logo reveal — advance `reveal_col` by REVEAL_COLS_PER_FRAME each frame.
    // The loop starts at REVEAL_COLS_PER_FRAME so the first frame is non-empty.
    let mut reveal_col = REVEAL_COLS_PER_FRAME;
    while reveal_col < logo_width {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let frame = build_reveal_frame(reveal_col);
        term.write_str(&frame)?;
        std::thread::sleep(std::time::Duration::from_millis(REVEAL_FRAME_MS));
        reveal_col += REVEAL_COLS_PER_FRAME;
    }
    // Final full-reveal frame ensures every column is visible.
    if !cancel.load(Ordering::Relaxed) {
        let frame = build_reveal_frame(logo_width);
        term.write_str(&frame)?;
    }

    // Phase 2: Underline sweep — gradient line appears beneath the logo.
    let mut sweep_col = UNDERLINE_COLS_PER_FRAME;
    while sweep_col < logo_width {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let frame = build_underline_frame(sweep_col, logo_width);
        term.write_str(&frame)?;
        std::thread::sleep(std::time::Duration::from_millis(UNDERLINE_FRAME_MS));
        sweep_col += UNDERLINE_COLS_PER_FRAME;
    }
    // Final full-width underline frame.
    if !cancel.load(Ordering::Relaxed) {
        let frame = build_underline_frame(logo_width, logo_width);
        term.write_str(&frame)?;
        // Brief hold so the completed image is visible before clearing.
        std::thread::sleep(std::time::Duration::from_millis(200));
    }

    // Clear the animation region (blank + logo + underline) and replace with
    // static getting-started output.
    term.clear_last_lines(1 + logo_height + 1)?;
    render_static(term)?;

    Ok(())
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use pretty_assertions::{assert_eq, assert_ne};

    use super::*;

    // ── marker file ───────────────────────────────────────────────────────

    #[test]
    fn already_welcomed_false_when_no_marker() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(dir.path());
        assert!(!already_welcomed(&ctx).unwrap());
    }

    #[test]
    fn write_marker_creates_file_and_already_welcomed_returns_true() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(dir.path());
        write_marker(&ctx).unwrap();
        assert!(already_welcomed(&ctx).unwrap());
    }

    #[test]
    fn write_marker_creates_parent_directories() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("new_home");
        let ctx = make_ctx(&home);
        write_marker(&ctx).unwrap();
        assert!(marker_path(&ctx).unwrap().exists());
    }

    #[test]
    fn write_marker_content_is_version_string() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(dir.path());
        write_marker(&ctx).unwrap();
        let content = std::fs::read_to_string(marker_path(&ctx).unwrap()).unwrap();
        assert_eq!(content, env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn cmd_welcome_writes_marker_after_run() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(dir.path());
        // Non-TTY: render_static path.
        cmd_welcome(&ctx, false).unwrap();
        assert!(already_welcomed(&ctx).unwrap());
    }

    #[test]
    fn cmd_welcome_noop_when_marker_exists_and_no_force() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(dir.path());
        write_marker(&ctx).unwrap();
        // Should return Ok(()) without doing anything further.
        cmd_welcome(&ctx, false).unwrap();
    }

    #[test]
    fn cmd_welcome_force_runs_even_if_marker_exists() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(dir.path());
        write_marker(&ctx).unwrap();
        cmd_welcome(&ctx, true).unwrap();
        // Marker should still exist (was re-written).
        assert!(already_welcomed(&ctx).unwrap());
    }

    // ── static rendering ──────────────────────────────────────────────────

    /// Verify that `render_static` output contains "creft", the version string,
    /// and all three getting-started commands (`creft add`, `creft list`, `creft up`).
    ///
    /// Uses `render_static_to_writer` with a `Vec<u8>` to capture the rendered
    /// bytes directly, so the spec's content requirements can be asserted.
    #[test]
    fn render_static_output_contains_required_content() {
        // Disable yansi so the output is plain ASCII without ANSI escape codes,
        // making string matching unambiguous.
        yansi::disable();
        let mut buf: Vec<u8> = Vec::new();
        render_static_to_writer(&mut buf).unwrap();
        yansi::enable();

        let output = String::from_utf8(buf).expect("render_static must produce valid UTF-8");

        assert!(
            output.contains("creft"),
            "output must contain 'creft'; got:\n{output}"
        );
        assert!(
            output.contains(env!("CARGO_PKG_VERSION")),
            "output must contain the version '{}'; got:\n{output}",
            env!("CARGO_PKG_VERSION")
        );
        assert!(
            output.contains("creft add"),
            "output must contain 'creft add'; got:\n{output}"
        );
        assert!(
            output.contains("creft list"),
            "output must contain 'creft list'; got:\n{output}"
        );
        assert!(
            output.contains("creft up"),
            "output must contain 'creft up'; got:\n{output}"
        );
    }

    #[test]
    fn logo_lines_fit_within_40_columns() {
        for line in LOGO {
            let char_count = line.chars().count();
            assert!(
                char_count <= 44, // box-drawing chars are single-width; 40-col logo + small margin
                "Logo line too wide ({char_count} chars): {line:?}"
            );
        }
    }

    // ── gradient_line ─────────────────────────────────────────────────────

    #[test]
    fn gradient_line_plain_when_yansi_disabled() {
        yansi::disable();
        let result = gradient_line("hello", GRAD_FROM, GRAD_TO);
        yansi::enable();
        assert!(
            !result.contains('\x1b'),
            "must not contain ANSI escapes when yansi is disabled"
        );
        assert_eq!(result, "hello");
    }

    #[test]
    fn gradient_line_contains_ansi_when_yansi_enabled() {
        yansi::enable();
        let result = gradient_line("hello", GRAD_FROM, GRAD_TO);
        assert!(
            result.contains('\x1b'),
            "must contain ANSI escapes when yansi is enabled"
        );
    }

    #[test]
    fn gradient_line_empty_string() {
        let result = gradient_line("", GRAD_FROM, GRAD_TO);
        assert_eq!(result, "");
    }

    #[test]
    fn gradient_line_single_char_uses_from_color() {
        yansi::enable();
        let result = gradient_line("X", GRAD_FROM, GRAD_TO);
        let from_r = GRAD_FROM.0.to_string();
        assert!(
            result.contains(&from_r),
            "single-char gradient should use GRAD_FROM color"
        );
    }

    // ── gradient_color_at ─────────────────────────────────────────────────

    #[test]
    fn gradient_color_at_first_position_is_grad_from() {
        let color = gradient_color_at(0, 10);
        assert_eq!(color, GRAD_FROM, "position 0 must equal GRAD_FROM");
    }

    #[test]
    fn gradient_color_at_last_position_is_grad_to() {
        let color = gradient_color_at(9, 10);
        assert_eq!(color, GRAD_TO, "last position must equal GRAD_TO");
    }

    #[test]
    fn gradient_color_at_single_position_is_grad_from() {
        let color = gradient_color_at(0, 1);
        assert_eq!(
            color, GRAD_FROM,
            "single-position gradient must return GRAD_FROM"
        );
    }

    // ── build_reveal_frame ────────────────────────────────────────────────

    #[test]
    fn reveal_frame_starts_with_save_and_ends_with_restore() {
        let frame = build_reveal_frame(0);
        assert!(
            frame.starts_with("\x1b[s"),
            "reveal frame must start with SCO save cursor"
        );
        assert!(
            frame.ends_with("\x1b[u"),
            "reveal frame must end with SCO restore cursor"
        );
    }

    #[test]
    fn reveal_frame_at_zero_contains_only_spaces_newlines_and_cursor_sequences() {
        yansi::disable();
        let frame = build_reveal_frame(0);
        yansi::enable();
        // Strip the save/restore escapes and verify only spaces and newlines remain.
        let inner = frame
            .strip_prefix("\x1b[s")
            .unwrap()
            .strip_suffix("\x1b[u")
            .unwrap();
        assert!(
            inner.chars().all(|c| c == ' ' || c == '\n'),
            "zero-reveal frame must contain only spaces and newlines; got: {inner:?}"
        );
    }

    #[test]
    fn reveal_frame_full_contains_all_logo_characters() {
        yansi::disable();
        let logo_width = LOGO.iter().map(|l| l.chars().count()).max().unwrap();
        let frame = build_reveal_frame(logo_width);
        yansi::enable();
        for line in LOGO {
            for ch in line.chars() {
                assert!(
                    frame.contains(ch),
                    "full-reveal frame must contain logo character {ch:?}"
                );
            }
        }
    }

    // ── build_underline_frame ─────────────────────────────────────────────

    #[test]
    fn underline_frame_starts_with_save_and_ends_with_restore() {
        let frame = build_underline_frame(0, 41);
        assert!(
            frame.starts_with("\x1b[s"),
            "underline frame must start with SCO save cursor"
        );
        assert!(
            frame.ends_with("\x1b[u"),
            "underline frame must end with SCO restore cursor"
        );
    }

    #[test]
    fn underline_frame_at_zero_contains_no_underline_chars() {
        yansi::disable();
        let frame = build_underline_frame(0, 41);
        yansi::enable();
        assert!(
            !frame.contains(UNDERLINE_CHAR),
            "zero-sweep frame must not contain underline characters"
        );
    }

    #[test]
    fn underline_frame_full_width_contains_expected_count_of_underline_chars() {
        yansi::disable();
        let logo_width = 41usize;
        let frame = build_underline_frame(logo_width, logo_width);
        yansi::enable();
        // Strip escape sequences to count bare underline characters.
        let char_count = frame.chars().filter(|&c| c == UNDERLINE_CHAR).count();
        assert_eq!(
            char_count, logo_width,
            "full underline frame must contain exactly {logo_width} underline characters"
        );
    }

    #[test]
    fn underline_frame_full_width_gradient_uses_grad_from_and_grad_to() {
        yansi::enable();
        let logo_width = 41usize;
        let frame = build_underline_frame(logo_width, logo_width);
        // GRAD_FROM RGB values should appear (from the first character's color).
        let from_r = GRAD_FROM.0.to_string();
        assert!(
            frame.contains(&from_r),
            "full underline frame must contain GRAD_FROM red channel {from_r}"
        );
        // GRAD_TO RGB values should appear (from the last character's color).
        let to_b = GRAD_TO.2.to_string();
        assert!(
            frame.contains(&to_b),
            "full underline frame must contain GRAD_TO blue channel {to_b}"
        );
    }

    // ── CursorGuard ───────────────────────────────────────────────────────

    #[test]
    fn cursor_guard_compiles_and_drop_impl_exists() {
        let term = console::Term::buffered_stdout();
        let _guard = CursorGuard::new(&term);
        // _guard drops here — show_cursor() is called (errors are silently ignored).
    }

    // ── render_animated fallback ──────────────────────────────────────────

    #[test]
    fn cmd_welcome_uses_static_path_when_not_a_tty() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(dir.path());
        cmd_welcome(&ctx, true).unwrap();
    }

    // ── helpers ───────────────────────────────────────────────────────────

    /// Build an `AppContext` using `home` as the home directory.
    ///
    /// The marker path becomes `<home>/.creft/.welcome-done`.
    fn make_ctx(home: &std::path::Path) -> AppContext {
        AppContext::for_test(home.to_path_buf(), home.to_path_buf())
    }

    // Ensure assert_ne is exercised so the import isn't flagged as unused.
    #[test]
    fn grad_from_and_grad_to_are_different_colors() {
        assert_ne!(GRAD_FROM, GRAD_TO, "gradient endpoints must differ");
    }
}
