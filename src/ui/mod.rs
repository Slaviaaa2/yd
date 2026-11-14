//! Codex-CLI-like styling helpers shared by the one-shot output and the interactive mode.
#![allow(dead_code)] // helpers also used by the interactive mode

pub mod ansi;
pub mod input;
pub mod oneshot;
pub mod picker;
pub mod term;

use std::io::IsTerminal;
use std::path::Path;
use std::sync::OnceLock;

use crossterm::style::Stylize;

static COLOR: OnceLock<bool> = OnceLock::new();
static TTY: OnceLock<bool> = OnceLock::new();

/// Decide once whether stdout gets colors / live redraw. Call early in `main`.
pub fn init() {
    let tty = std::io::stdout().is_terminal();
    let no_color = std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty());
    let dumb = std::env::var("TERM").is_ok_and(|t| t == "dumb");
    let _ = TTY.set(tty);
    let _ = COLOR.set(tty && !no_color && !dumb);
}

pub fn color_enabled() -> bool {
    *COLOR.get().unwrap_or(&false)
}

/// stdout is an interactive terminal: live redraw allowed.
pub fn is_tty() -> bool {
    *TTY.get().unwrap_or(&false)
}

macro_rules! painter {
    ($name:ident, $method:ident) => {
        pub fn $name(s: impl AsRef<str>) -> String {
            if color_enabled() {
                s.as_ref().$method().to_string()
            } else {
                s.as_ref().to_string()
            }
        }
    };
}
painter!(dim, dim);
painter!(bold, bold);
painter!(cyan, cyan);
painter!(green, green);
painter!(red, red);
painter!(yellow, yellow);

pub const BULLET: &str = "\u{2022}";
pub const BRANCH: &str = "\u{2514}";
pub const CHECK: &str = "\u{2714}";
pub const CROSS: &str = "\u{2718}";
const SPINNER: [&str; 10] = [
    "\u{280b}", "\u{2819}", "\u{2839}", "\u{2838}", "\u{283c}", "\u{2834}", "\u{2826}", "\u{2827}", "\u{2807}",
    "\u{280f}",
];

/// `• Title` (dim bullet, bold text).
pub fn heading(text: &str) -> String {
    format!("{} {}", dim(BULLET), bold(text))
}

/// `  └ detail` (all dim).
pub fn detail(text: &str) -> String {
    dim(format!("  {BRANCH} {text}"))
}

pub fn ok_line(text: &str) -> String {
    format!("{} {}", green(CHECK), text)
}

pub fn error_line(text: &str) -> String {
    format!("{} {}", red(CROSS), text)
}

pub fn warn_line(text: &str) -> String {
    yellow(format!("! {text}"))
}

pub fn spinner_frame(tick: usize) -> String {
    cyan(SPINNER[tick % SPINNER.len()])
}

/// `━━━━╺━━━━` bar: cyan done part, dim track.
pub fn progress_bar(percent: f64, width: usize) -> String {
    let p = (percent / 100.0).clamp(0.0, 1.0);
    let filled = ((p * width as f64) as usize).min(width);
    if filled >= width {
        return cyan("\u{2501}".repeat(width));
    }
    let head = cyan(format!("{}\u{257a}", "\u{2501}".repeat(filled)));
    let rest = dim("\u{2501}".repeat(width - filled - 1));
    format!("{head}{rest}")
}

/// Lines of `yd fav ls` / `/fav`: favorites, then recent directories.
pub fn fav_list_lines(cfg: &crate::config::Config) -> Vec<String> {
    let mut out = vec![heading("Favorites")];
    if cfg.favorites.is_empty() {
        out.push(detail("none (yd fav add <name> [path])"));
    }
    for f in &cfg.favorites {
        out.push(detail(&format!("{}  {}", f.name, abbrev_home(&f.path))));
    }
    if !cfg.recent.is_empty() {
        out.push(heading("Recent"));
        for r in &cfg.recent {
            out.push(detail(&abbrev_home(r)));
        }
    }
    out
}

/// Replace a leading home dir with `~`.
pub fn abbrev_home(p: &Path) -> String {
    if let Some(home) = dirs::home_dir()
        && let Ok(rest) = p.strip_prefix(&home)
    {
        return if rest.as_os_str().is_empty() {
            "~".into()
        } else {
            format!("~/{}", rest.display())
        };
    }
    p.display().to_string()
}

/// `3:33`, `1:02:03`.
pub fn fmt_duration(secs: f64) -> String {
    let s = secs.round().max(0.0) as u64;
    let (h, m, s) = (s / 3600, (s / 60) % 60, s % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// `7.9 MiB`.
pub fn fmt_bytes(n: u64) -> String {
    const U: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", U[i])
    }
}

pub fn fmt_speed(bytes_per_sec: f64) -> String {
    format!("{}/s", fmt_bytes(bytes_per_sec.max(0.0) as u64))
}

/// Short elapsed time: `12s`, `1m05s`.
pub fn fmt_elapsed(secs: u64) -> String {
    if secs >= 60 {
        format!("{}m{:02}s", secs / 60, secs % 60)
    } else {
        format!("{secs}s")
    }
}

/// Truncate to `max` display columns with an ellipsis (plain text only, no ANSI).
pub fn truncate(s: &str, max: usize) -> String {
    use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
    if s.width() <= max {
        return s.to_string();
    }
    let mut out = String::new();
    let mut w = 0;
    for ch in s.chars() {
        let cw = ch.width().unwrap_or(0);
        if w + cw + 1 > max {
            break;
        }
        out.push(ch);
        w += cw;
    }
    out.push('\u{2026}');
    out
}

pub fn term_width() -> usize {
    crossterm::terminal::size()
        .map(|(w, _)| w as usize)
        .unwrap_or(80)
        .max(20)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats() {
        assert_eq!(fmt_duration(213.0), "3:33");
        assert_eq!(fmt_duration(3723.0), "1:02:03");
        assert_eq!(fmt_bytes(8_283_750), "7.9 MiB");
        assert_eq!(fmt_bytes(512), "512 B");
        assert_eq!(fmt_elapsed(65), "1m05s");
        assert_eq!(truncate("abcdef", 4), "abc\u{2026}");
        assert_eq!(truncate("abc", 4), "abc");
    }

    #[test]
    fn bar_width() {
        use unicode_width::UnicodeWidthStr;
        for p in [0.0, 33.0, 99.9, 100.0] {
            assert_eq!(progress_bar(p, 20).width(), 20);
        }
    }
}
