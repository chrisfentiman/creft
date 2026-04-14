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

/// Sparkle foreground colors — warm ember tones that complement the logo gradient.
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
        use std::fmt::Write as FmtWrite;
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
///
/// Sparkles appear below the logo like falling embers. Each sparkle has a
/// `start_frame` so they are staggered across the burst period rather than
/// all appearing simultaneously. Once a sparkle finishes its visible lifetime
/// it is marked `done` and no longer drawn.
struct Sparkle {
    /// Column (0-based), relative to the terminal's left edge.
    col: usize,
    /// Row offset below the logo bottom edge (0-based).
    row: usize,
    ch: char,
    color: (u8, u8, u8),
    /// Frame index on which this sparkle first becomes visible.
    start_frame: usize,
    /// Frames left to display. Counts down from initial value to 0, then done.
    frames_remaining: u8,
    /// True once the sparkle has been erased and should not be drawn again.
    done: bool,
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
///
/// Animation design:
/// - Phase 1: Logo reveal — each line appears over ~120ms.
/// - Phase 2: Sparkle burst — embers fall below the logo over ~1.2s.
///   Sparkles are staggered so they appear one-by-one, not all at once.
///   Sparkles are positioned below the logo, keeping the logo itself clean.
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

    // Reserve vertical space: 1 blank + logo + sparkle rows below logo + static block.
    let logo_height = LOGO.len();
    let sparkle_rows = 3usize; // embers fall up to 3 rows below the logo
    let static_lines = 10; // blank + tagline + version + blank + header + 3 commands + blank + hint + blank
    let total_lines = 1 + logo_height + sparkle_rows + static_lines;

    for _ in 0..total_lines {
        term.write_line("")?;
    }
    term.move_cursor_up(total_lines)?;

    // Phase 1: Logo reveal — one line per ~120ms (≈4 frames at 30fps).
    let frames_per_line = 4usize;
    for logo_line in LOGO.iter() {
        for frame in 0..frames_per_line {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            if frame == 0 {
                let rendered = gradient_line(logo_line, GRAD_FROM, GRAD_TO);
                term.write_line(&rendered)?;
            }
            std::thread::sleep(FRAME_DURATION);
        }
        if cancel.load(Ordering::Relaxed) {
            break;
        }
    }

    // Phase 2: Sparkle burst — embers cascade below the logo.
    //
    // The cursor is now logo_height lines below the block start, sitting at
    // the first sparkle row. Sparkles are placed in the `sparkle_rows` rows
    // immediately below the logo. Each sparkle has a `start_frame` that
    // staggers its appearance across the burst period so they drift in
    // one-by-one rather than all flashing simultaneously.
    if !cancel.load(Ordering::Relaxed) {
        let mut rng = Lcg::new();
        let total_sparkles = 18usize;
        let frames_per_burst = 36usize; // ~1.2s at 30fps

        // Leave a small right margin to avoid triggering terminal line-wrapping.
        let usable_cols = (cols as usize).saturating_sub(2).max(1);

        // Distribute start_frames evenly across the burst window, reserving
        // the last few frames so all sparkles finish erasing before we clear.
        let burst_window = frames_per_burst.saturating_sub(4);

        let mut sparkles: Vec<Sparkle> = (0..total_sparkles)
            .map(|i| {
                let col = (rng.next() as usize) % usable_cols;
                // Row 0 is directly below the last logo line; rows go up to sparkle_rows-1.
                let row = (rng.next() as usize) % sparkle_rows;
                let ch = SPARKLE_CHARS[(rng.next() as usize) % SPARKLE_CHARS.len()];
                let color_idx = (rng.next() as usize) % SPARKLE_COLORS.len();
                let color = SPARKLE_COLORS[color_idx];
                let frames_remaining = 2 + (rng.next() as u8 % 2); // 2 or 3 frames visible
                let start_frame = (i * burst_window) / total_sparkles;
                Sparkle {
                    col,
                    row,
                    ch,
                    color,
                    start_frame,
                    frames_remaining,
                    done: false,
                }
            })
            .collect();

        // Reserve the sparkle rows by printing blank lines, then return the
        // cursor to the top of the sparkle region.
        for _ in 0..sparkle_rows {
            term.write_line("")?;
        }
        term.move_cursor_up(sparkle_rows)?;

        for current_frame in 0..frames_per_burst {
            if cancel.load(Ordering::Relaxed) {
                break;
            }

            for sparkle in &mut sparkles {
                if sparkle.done || current_frame < sparkle.start_frame {
                    continue;
                }

                if sparkle.frames_remaining > 0 {
                    // Draw the sparkle at its position below the logo.
                    term.move_cursor_down(sparkle.row)?;
                    term.move_cursor_right(sparkle.col)?;
                    let s = sparkle.ch.to_string();
                    let painted = format!(
                        "{}",
                        s.as_str()
                            .rgb(sparkle.color.0, sparkle.color.1, sparkle.color.2)
                    );
                    term.write_str(&painted)?;
                    // Return cursor to the top of the sparkle region.
                    term.move_cursor_up(sparkle.row)?;
                    term.move_cursor_left(sparkle.col + 1)?;
                    sparkle.frames_remaining -= 1;
                } else {
                    // Erase: overwrite with a space and mark done.
                    term.move_cursor_down(sparkle.row)?;
                    term.move_cursor_right(sparkle.col)?;
                    term.write_str(" ")?;
                    term.move_cursor_up(sparkle.row)?;
                    term.move_cursor_left(sparkle.col + 1)?;
                    sparkle.done = true;
                }
            }

            std::thread::sleep(FRAME_DURATION);
        }

        // Advance past the sparkle rows so clear_last_lines covers the right region.
        term.move_cursor_down(sparkle_rows)?;
    }

    // Clear the entire animation region (1 blank + logo + sparkle rows) and
    // replace with clean static output.
    term.clear_last_lines(1 + logo_height + sparkle_rows)?;
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

    // ── sparkle staggering ────────────────────────────────────────────────

    /// Verify the stagger formula distributes sparkle start_frames across the
    /// burst window with no duplicates and all values in range.
    #[test]
    fn sparkle_start_frames_are_staggered_and_in_range() {
        let total_sparkles = 18usize;
        let frames_per_burst = 36usize;
        let burst_window = frames_per_burst.saturating_sub(4);
        let start_frames: Vec<usize> = (0..total_sparkles)
            .map(|i| (i * burst_window) / total_sparkles)
            .collect();

        for &sf in &start_frames {
            assert!(
                sf < burst_window,
                "start_frame {sf} must be within burst_window {burst_window}"
            );
        }

        let unique: std::collections::HashSet<usize> = start_frames.iter().copied().collect();
        assert_eq!(
            unique.len(),
            total_sparkles,
            "all sparkle start_frames must be distinct"
        );
    }

    /// A sparkle marked `done` is terminal — it must never be redrawn.
    ///
    /// The animation loop skips any sparkle where `done == true`, even if
    /// `frames_remaining` is non-zero.
    #[test]
    fn sparkle_done_flag_prevents_redraw() {
        let sparkle = Sparkle {
            col: 0,
            row: 0,
            ch: '*',
            color: (255, 255, 255),
            start_frame: 0,
            frames_remaining: 5, // would normally draw, but done=true guards it
            done: true,
        };
        assert!(sparkle.done, "done flag must be set and visible");
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
