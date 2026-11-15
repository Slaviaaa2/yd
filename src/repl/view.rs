//! Pure rendering helpers for the REPL: startup card, progress line, command popup.

use std::path::Path;

use unicode_width::UnicodeWidthStr;

use super::suggest::Cand;
use crate::format::FormatSpec;
use crate::ui::{self, ansi, bold, cyan, dim, progress_bar, spinner_frame, truncate, yellow};

const CARD_WIDTH: usize = 49;
const MAX_SUGGEST_ROWS: usize = 6;

/// Startup card + hint line.
pub fn card(width: usize, spec: &FormatSpec, dest: &Path, fav: Option<&str>) -> Vec<String> {
    let w = CARD_WIDTH.min(width.max(24));
    let inner = w - 2;
    let content_w = inner - 2;
    let row = |content: String| -> String {
        let pad = content_w.saturating_sub(ansi::width(&content));
        format!("{} {content}{} {}", dim("\u{2502}"), " ".repeat(pad), dim("\u{2502}"))
    };
    let kv = |label: &str, value: String, value_w: usize, hint: &str| -> String {
        let lab = format!("{label:<10}");
        let room = content_w.saturating_sub(lab.width());
        let (value, value_w) = if value_w > room {
            let t = truncate(&strip(&value), room);
            let w = t.width();
            (t, w)
        } else {
            (value, value_w)
        };
        let gap = room.saturating_sub(value_w + hint.width());
        let hint = if gap >= 2 { dim(hint) } else { String::new() };
        let fill = if gap >= 2 { gap } else { 0 };
        row(format!("{}{value}{}{hint}", dim(&lab), " ".repeat(fill)))
    };
    let fmt = spec.to_string();
    let path = ui::abbrev_home(dest);
    let (dest_val, dest_w) = match fav {
        Some(name) => (
            format!("{path}  {} {}", yellow("\u{2605}"), dim(name)),
            path.width() + 4 + name.width(),
        ),
        None => (path.clone(), path.width()),
    };
    vec![
        dim(format!("\u{256d}{}\u{256e}", "\u{2500}".repeat(inner))),
        row(format!(
            "{} {}",
            bold(">_ yd"),
            dim(format!("(v{})", env!("CARGO_PKG_VERSION")))
        )),
        row(String::new()),
        kv("format:", fmt.clone(), fmt.width(), "/f to change"),
        kv("save to:", dest_val, dest_w, "/o to change"),
        dim(format!("\u{2570}{}\u{256f}", "\u{2500}".repeat(inner))),
        dim("  Paste a URL and press Enter \u{b7} /help for commands"),
    ]
}

fn strip(s: &str) -> String {
    ansi::to_line(s).spans.iter().map(|sp| sp.content.as_ref()).collect()
}

pub struct Progress<'a> {
    pub tick: usize,
    pub title: &'a str,
    pub percent: Option<f64>,
    pub speed: Option<f64>,
    pub eta: Option<u64>,
    /// Post-processing label (replaces the percentage).
    pub stage: Option<&'a str>,
    pub queued: usize,
}

/// `  ⠹ Title  58% ━━━━━╺━━━  4.2 MiB/s · 0:05  +2 queued`, fitted to `width` columns.
pub fn progress_line(p: &Progress, width: usize) -> String {
    let lead = format!("  {} ", spinner_frame(p.tick));
    let queued = if p.queued > 0 { format!("  +{} queued", p.queued) } else { String::new() };
    let mut tail_parts = Vec::new();
    if let Some(s) = p.speed {
        tail_parts.push(ui::fmt_speed(s));
    }
    if let Some(e) = p.eta {
        tail_parts.push(ui::fmt_duration(e as f64));
    }
    let tail = if tail_parts.is_empty() { String::new() } else { format!("  {}", tail_parts.join(" \u{b7} ")) };

    let mut bar_w = ui::oneshot::BAR_WIDTH;
    let mut show_tail = true;
    let mut show_queued = true;
    let min_title = 8;
    let has_bar = p.percent.is_some() && p.stage.is_none();
    let fixed = |bar_w: usize, show_tail: bool, show_queued: bool| -> usize {
        4 + if has_bar { 2 + 4 + 1 + bar_w } else { 0 }
            + if has_bar && show_tail { tail.width() } else { 0 }
            + if show_queued { queued.width() } else { 0 }
    };
    if width.saturating_sub(fixed(bar_w, show_tail, show_queued)) < min_title {
        show_tail = false;
    }
    if width.saturating_sub(fixed(bar_w, show_tail, show_queued)) < min_title {
        bar_w = 10;
    }
    if width.saturating_sub(fixed(bar_w, show_tail, show_queued)) < min_title {
        show_queued = false;
    }
    let stage_text = p.stage.map(|s| format!("  {s}\u{2026}")).unwrap_or_default();
    let budget = width.saturating_sub(fixed(bar_w, show_tail, show_queued) + stage_text.width()).max(1);
    let mut line = format!("{lead}{}", truncate(p.title, budget));
    if !stage_text.is_empty() {
        line.push_str(&dim(&stage_text));
    }
    if let (true, Some(pct)) = (has_bar, p.percent) {
        line.push_str(&format!("  {:>3}% {}", pct as u32, progress_bar(pct, bar_w)));
        if show_tail && !tail.is_empty() {
            line.push_str(&dim(&tail));
        }
    }
    if show_queued && !queued.is_empty() {
        line.push_str(&dim(&queued));
    }
    line
}

/// Rows of the suggestion list (selected row highlighted), at most `MAX_SUGGEST_ROWS` around the selection.
pub fn suggest_lines(items: &[Cand], sel: usize, width: usize) -> Vec<String> {
    let rows = items.len().min(MAX_SUGGEST_ROWS);
    let top = if sel < rows { 0 } else { sel + 1 - rows };
    let shown = &items[top..top + rows];
    let label_w = shown.iter().map(|c| c.label.width()).max().unwrap_or(0).min(width.saturating_sub(12).max(8));
    shown
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let label = truncate(&c.label, label_w);
            let pad = " ".repeat(label_w.saturating_sub(label.width()));
            let rest = truncate(&c.detail, width.saturating_sub(label_w + 6));
            if top + i == sel {
                format!("  {}{pad}  {}", cyan(&label), cyan(&rest))
            } else {
                dim(format!("  {label}{pad}  {rest}"))
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn card_rows_have_equal_width() {
        let spec = FormatSpec::parse("mp3:192").unwrap();
        for w in [30, 49, 100] {
            let c = card(w, &spec, Path::new("/very/long/path/that/does/not/fit/in/the/card/at/all"), Some("music"));
            let widths: Vec<usize> = c[..6].iter().map(|l| ansi::width(l)).collect();
            assert!(widths.iter().all(|&x| x == widths[0]), "{widths:?}");
            assert!(widths[0] <= w.max(24));
        }
        let c = card(100, &spec, Path::new("/tmp"), None);
        assert!(strip(&c[3]).contains("mp3 \u{b7} 192k"));
        assert!(strip(&c[3]).contains("/f to change"));
        assert!(strip(&c[4]).contains("/tmp"));
    }

    #[test]
    fn progress_fits_width() {
        for w in [24, 30, 40, 60, 80, 120] {
            let p = Progress {
                tick: 3,
                title: "A very long title that keeps going and going \u{65e5}\u{672c}\u{8a9e}\u{306e}\u{30bf}\u{30a4}\u{30c8}\u{30eb}",
                percent: Some(58.0),
                speed: Some(4.2 * 1024.0 * 1024.0),
                eta: Some(5),
                stage: None,
                queued: 2,
            };
            assert!(ansi::width(&progress_line(&p, w)) <= w, "width {w}");
        }
        let p = Progress {
            tick: 0,
            title: "Me at the zoo",
            percent: Some(58.0),
            speed: Some(1024.0),
            eta: Some(5),
            stage: None,
            queued: 0,
        };
        let s = strip(&progress_line(&p, 100));
        assert!(s.contains("Me at the zoo") && s.contains("58%") && !s.contains("queued"));
        let p = Progress {
            stage: Some("Converting audio"),
            percent: None,
            ..p
        };
        assert!(strip(&progress_line(&p, 100)).contains("Converting audio\u{2026}"));
    }
}
