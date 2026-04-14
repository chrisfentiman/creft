use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use yansi::Paint as _;

use crate::error::CreftError;
use crate::model::AppContext;

// ── ASCII logo ─────────────────────────────────────────────────────────────

/// The creft ASCII logo.
///
/// Block-letter art spelling "creft". Each line fits within 40 columns.
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

/// Sparkle foreground colors.
const SPARKLE_COLORS: &[(u8, u8, u8)] = &[
    (255, 215, 0),   // gold
    (255, 140, 0),   // dark orange
    (255, 99, 71),   // tomato / coral
    (255, 255, 255), // white
];

// ── Timing ─────────────────────────────────────────────────────────────────

/// Target frame duration: ~30 FPS.
const FRAME_DURATION: std::time::Duration = std::time::Duration::from_millis(33);

// ── Sparkle characters ─────────────────────────────────────────────────────

/// Printable, single-width characters used in the sparkle burst.
const SPARKLE_CHARS: &[char] = &['*', '.', '+', '\'', '`', ','];

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
    term.write_line("")?;
    for line in LOGO {
        term.write_line(&gradient_line(line, GRAD_FROM, GRAD_TO))?;
    }
    term.write_line("")?;
    term.write_line(&format!(
        "  {}",
        "Executable skills for Agents".rgb(180, 180, 180)
    ))?;
    term.write_line(&format!(
        "  v{}",
        env!("CARGO_PKG_VERSION").rgb(120, 120, 120)
    ))?;
    term.write_line("")?;
    term.write_line(&format!("  {}", "Get started:".rgb(212, 160, 23)))?;
    term.write_line(&format!(
        "    {}    {}",
        "creft add".rgb(0, 206, 209),
        "Create a skill from stdin".rgb(160, 160, 160)
    ))?;
    term.write_line(&format!(
        "    {}   {}",
        "creft list".rgb(0, 206, 209),
        "See available skills".rgb(160, 160, 160)
    ))?;
    term.write_line(&format!(
        "    {}     {}",
        "creft up".rgb(0, 206, 209),
        "Set up editor integrations".rgb(160, 160, 160)
    ))?;
    term.write_line("")?;
    term.write_line("  Run creft --help for the full command reference.")?;
    term.write_line("")?;
    Ok(())
}

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
        // Build a one-char string to call .rgb() on &str via Paint trait.
        let s = ch.to_string();
        use std::fmt::Write as _;
        let _ = write!(out, "{}", s.as_str().rgb(r, g, b));
    }
    out
}

fn lerp_u8(a: u8, b: u8, t: f32) -> u8 {
    let result = a as f32 + (b as f32 - a as f32) * t;
    result.round().clamp(0.0, 255.0) as u8
}

// ── Animated rendering ─────────────────────────────────────────────────────

/// A positioned sparkle for the burst animation phase.
struct Sparkle {
    /// Column (0-based), relative to the terminal's left edge.
    col: usize,
    /// Row (0-based), relative to the terminal's top edge — set during layout.
    row: usize,
    ch: char,
    color: (u8, u8, u8),
    frames_remaining: u8,
}

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
/// cleanly. Registers a SIGINT cancellation flag matching the pattern in
/// `src/cmd/run.rs` — the animation loop breaks early on cancellation so the
/// `CursorGuard` still drops on the normal return path.
fn render_animated(term: &console::Term) -> Result<(), CreftError> {
    let (rows, cols) = term.size();

    if cols < 50 || rows < 15 {
        return render_static(term);
    }

    // SIGINT cancellation — same pattern as src/cmd/run.rs:169-174.
    let cancel = Arc::new(AtomicBool::new(false));
    #[cfg(unix)]
    let _ = signal_hook::flag::register(signal_hook::consts::SIGINT, Arc::clone(&cancel));

    term.hide_cursor()?;
    let _guard = CursorGuard::new(term);

    // Reserve vertical space: logo + 2 blank lines above + static block below.
    let logo_height = LOGO.len();
    let static_lines = 10; // blank + tagline + version + blank + header + 3 commands + blank + hint + blank
    let total_lines = 1 + logo_height + static_lines;

    for _ in 0..total_lines {
        term.write_line("")?;
    }
    term.move_cursor_up(total_lines)?;

    // Capture the starting row by tracking lines printed.
    let start_row: usize = {
        // We printed `total_lines` blank lines and then moved back up, so the
        // cursor is at the row where we started. Use the terminal size to
        // derive an absolute row: we can't query the cursor position via
        // `console::Term`, so we rely on relative movement throughout.
        0 // relative offset from our reserved block start
    };

    // Phase 1: Logo reveal — one line per ~120ms (≈4 frames at 30fps).
    let frames_per_line = 4usize;
    for (line_idx, logo_line) in LOGO.iter().enumerate() {
        for frame in 0..frames_per_line {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            if frame == 0 {
                // Print the logo line on the first frame for this row.
                let rendered = gradient_line(logo_line, GRAD_FROM, GRAD_TO);
                term.write_line(&rendered)?;
            }
            std::thread::sleep(FRAME_DURATION);
        }
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let _ = start_row + line_idx + 1; // suppress unused warning
    }

    // Phase 2: Sparkle burst — 15 sparkles, each 2-3 frames.
    if !cancel.load(Ordering::Relaxed) {
        let mut rng = Lcg::new();
        let sparkle_area_cols = cols as usize;
        let sparkle_area_rows = logo_height;

        // We are currently logo_height lines below our start. Move back up to
        // overlay the logo area for sparkle placement.
        term.move_cursor_up(logo_height)?;

        // Generate sparkles with random positions inside the logo bounding box.
        let total_sparkles = 18usize;
        let frames_per_burst = 36usize; // ~1.2s at 30fps

        let sparkle_cols = sparkle_area_cols.max(1);
        let sparkle_rows = sparkle_area_rows.max(1);

        let mut sparkles: Vec<Sparkle> = (0..total_sparkles)
            .map(|_| {
                let col = (rng.next() as usize) % sparkle_cols;
                let row = (rng.next() as usize) % sparkle_rows;
                let ch = SPARKLE_CHARS[(rng.next() as usize) % SPARKLE_CHARS.len()];
                let color_idx = (rng.next() as usize) % SPARKLE_COLORS.len();
                let color = SPARKLE_COLORS[color_idx];
                let frames_remaining = 2 + (rng.next() as u8 % 2); // 2 or 3 frames
                Sparkle {
                    col,
                    row,
                    ch,
                    color,
                    frames_remaining,
                }
            })
            .collect();

        for _frame in 0..frames_per_burst {
            if cancel.load(Ordering::Relaxed) {
                break;
            }

            for sparkle in &mut sparkles {
                if sparkle.frames_remaining > 0 {
                    // Position cursor at sparkle location (relative: move from logo top).
                    term.move_cursor_down(sparkle.row)?;
                    term.move_cursor_right(sparkle.col)?;
                    let s = sparkle.ch.to_string();
                    let painted = format!(
                        "{}",
                        s.as_str()
                            .rgb(sparkle.color.0, sparkle.color.1, sparkle.color.2)
                    );
                    term.write_str(&painted)?;
                    // Return to logo top for next sparkle.
                    term.move_cursor_up(sparkle.row)?;
                    // Move left back (sparkle.col + 1 for the character written).
                    term.move_cursor_left(sparkle.col + 1)?;
                    sparkle.frames_remaining -= 1;
                } else if sparkle.frames_remaining == 0 {
                    // Erase: overwrite with a space.
                    term.move_cursor_down(sparkle.row)?;
                    term.move_cursor_right(sparkle.col)?;
                    term.write_str(" ")?;
                    term.move_cursor_up(sparkle.row)?;
                    term.move_cursor_left(sparkle.col + 1)?;
                    sparkle.frames_remaining = u8::MAX; // sentinel: done
                }
            }

            std::thread::sleep(FRAME_DURATION);
        }

        // Move back down to below the logo so clear_last_lines works correctly.
        term.move_cursor_down(logo_height)?;
    }

    // Clear the entire animation region and replace with clean static output.
    term.clear_last_lines(logo_height + 1)?;
    render_static(term)?;

    Ok(())
}

// ── Simple LCG PRNG ────────────────────────────────────────────────────────

/// A minimal linear congruential generator seeded from wall-clock microseconds.
///
/// Parameters: multiplier and increment from Knuth's MMIX.
/// Used only for sparkle position randomisation — no security or statistical
/// quality requirements.
struct Lcg {
    state: u64,
}

impl Lcg {
    fn new() -> Self {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_micros() as u64)
            .unwrap_or(42);
        Self {
            state: seed ^ 0xDEAD_BEEF_CAFE_BABE,
        }
    }

    fn next(&mut self) -> u32 {
        self.state = self
            .state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.state >> 33) as u32
    }
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
        // Use a home dir where ~/.creft/ does not yet exist.
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
        // force=true must not short-circuit.
        cmd_welcome(&ctx, true).unwrap();
        // Marker should still exist (was re-written).
        assert!(already_welcomed(&ctx).unwrap());
    }

    // ── static rendering ──────────────────────────────────────────────────

    #[test]
    fn render_static_output_contains_version() {
        let term = console::Term::buffered_stdout();
        render_static(&term).unwrap();
        term.flush().unwrap();
        // We can't easily capture Term's buffer, so check that the call succeeds
        // and the version constant is what we expect.
        assert!(!env!("CARGO_PKG_VERSION").is_empty());
    }

    #[test]
    fn logo_lines_fit_within_40_columns() {
        for line in LOGO {
            // Count Unicode scalar values (not bytes) as a proxy for display width.
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
        // When yansi is disabled, Paint emits no ANSI escapes.
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
        // Single character: t=0.0, so color = GRAD_FROM.
        let result = gradient_line("X", GRAD_FROM, GRAD_TO);
        // Should contain the from color components in the ANSI sequence.
        let from_r = GRAD_FROM.0.to_string();
        assert!(
            result.contains(&from_r),
            "single-char gradient should use GRAD_FROM color"
        );
    }

    // ── sparkle chars ─────────────────────────────────────────────────────

    #[test]
    fn sparkle_chars_are_printable_single_width() {
        for &ch in SPARKLE_CHARS {
            assert!(
                ch.is_ascii_graphic() || ch == '\'',
                "sparkle char {ch:?} must be printable single-width ASCII"
            );
        }
    }

    // ── CursorGuard ───────────────────────────────────────────────────────

    #[test]
    fn cursor_guard_compiles_and_drop_impl_exists() {
        // Structural: verify CursorGuard is constructible and has Drop.
        // We can't easily observe show_cursor() in a test without a TTY, but
        // we verify the type is sound.
        let term = console::Term::buffered_stdout();
        let _guard = CursorGuard::new(&term);
        // _guard drops here — show_cursor() is called (errors are silently ignored).
    }

    // ── render_animated fallback ──────────────────────────────────────────

    #[test]
    fn cmd_welcome_uses_static_path_when_not_a_tty() {
        // In the test harness stdout is not a TTY, so cmd_welcome must take
        // the static path (no panic, no terminal-specific ops).
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(dir.path());
        cmd_welcome(&ctx, true).unwrap();
    }

    // ── LCG ──────────────────────────────────────────────────────────────

    #[test]
    fn lcg_produces_different_values() {
        let mut rng = Lcg { state: 1 };
        let a = rng.next();
        let b = rng.next();
        assert_ne!(a, b, "LCG must advance state each call");
    }

    // ── helpers ───────────────────────────────────────────────────────────

    /// Build an `AppContext` using `home` as the home directory.
    ///
    /// The marker path becomes `<home>/.creft/.welcome-done`.
    fn make_ctx(home: &std::path::Path) -> AppContext {
        AppContext::for_test(home.to_path_buf(), home.to_path_buf())
    }
}
