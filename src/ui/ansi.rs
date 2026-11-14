//! Bridge between the ANSI strings produced by the `ui` helpers and ratatui `Line`s.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthChar;

fn apply_sgr(style: &mut Style, params: &str) {
    let nums: Vec<u16> = if params.is_empty() {
        vec![0]
    } else {
        params.split(';').map(|p| p.parse().unwrap_or(0)).collect()
    };
    let mut i = 0;
    while i < nums.len() {
        match nums[i] {
            0 => *style = Style::default(),
            1 => *style = style.add_modifier(Modifier::BOLD),
            2 => *style = style.add_modifier(Modifier::DIM),
            3 => *style = style.add_modifier(Modifier::ITALIC),
            4 => *style = style.add_modifier(Modifier::UNDERLINED),
            22 => *style = style.remove_modifier(Modifier::BOLD | Modifier::DIM),
            23 => *style = style.remove_modifier(Modifier::ITALIC),
            24 => *style = style.remove_modifier(Modifier::UNDERLINED),
            n @ 30..=37 => *style = style.fg(Color::Indexed((n - 30) as u8)),
            n @ 90..=97 => *style = style.fg(Color::Indexed((n - 90 + 8) as u8)),
            39 => *style = style.fg(Color::Reset),
            n @ (38 | 48) => {
                let color = match nums.get(i + 1) {
                    Some(5) if i + 2 < nums.len() => {
                        i += 2;
                        Some(Color::Indexed(nums[i] as u8))
                    }
                    Some(2) if i + 4 < nums.len() => {
                        i += 4;
                        Some(Color::Rgb(nums[i - 2] as u8, nums[i - 1] as u8, nums[i] as u8))
                    }
                    _ => None,
                };
                if let Some(c) = color {
                    *style = if n == 38 { style.fg(c) } else { style.bg(c) };
                }
            }
            _ => {}
        }
        i += 1;
    }
}

/// Parse an SGR-styled string into a ratatui line. Non-SGR escape sequences are dropped.
pub fn to_line(s: &str) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut style = Style::default();
    let mut buf = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' && chars.peek() == Some(&'[') {
            chars.next();
            let mut params = String::new();
            let mut fin = None;
            for p in chars.by_ref() {
                if ('\u{40}'..='\u{7e}').contains(&p) {
                    fin = Some(p);
                    break;
                }
                params.push(p);
            }
            if fin == Some('m') {
                if !buf.is_empty() {
                    spans.push(Span::styled(std::mem::take(&mut buf), style));
                }
                apply_sgr(&mut style, &params);
            }
        } else {
            buf.push(c);
        }
    }
    if !buf.is_empty() {
        spans.push(Span::styled(buf, style));
    }
    Line::from(spans)
}

/// Display width of an ANSI string (escape sequences ignored).
pub fn width(s: &str) -> usize {
    to_line(s).spans.iter().map(|sp| sp.content.chars().map(|c| c.width().unwrap_or(0)).sum::<usize>()).sum()
}

/// Hard-wrap a line at `width` display columns (character granularity). Always returns >= 1 line.
pub fn wrap(line: Line<'static>, width: usize) -> Vec<Line<'static>> {
    let width = width.max(1);
    let mut out: Vec<Vec<Span<'static>>> = vec![Vec::new()];
    let mut used = 0usize;
    for span in line.spans {
        let mut chunk = String::new();
        for c in span.content.chars() {
            let cw = c.width().unwrap_or(0);
            if used + cw > width && used > 0 {
                if !chunk.is_empty() {
                    out.last_mut().unwrap().push(Span::styled(std::mem::take(&mut chunk), span.style));
                }
                out.push(Vec::new());
                used = 0;
            }
            chunk.push(c);
            used += cw;
        }
        if !chunk.is_empty() {
            out.last_mut().unwrap().push(Span::styled(chunk, span.style));
        }
    }
    out.into_iter().map(Line::from).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_styles() {
        let l = to_line("\x1b[2mdim\x1b[0m plain \x1b[38;5;14mcy\x1b[0m");
        assert_eq!(l.spans.len(), 3);
        assert!(l.spans[0].style.add_modifier.contains(Modifier::DIM));
        assert_eq!(l.spans[1].style, Style::default());
        assert_eq!(l.spans[2].style.fg, Some(Color::Indexed(14)));
        assert_eq!(width("\x1b[1mab\x1b[0mc"), 3);
    }

    #[test]
    fn roundtrips_crossterm_output() {
        use crossterm::style::Stylize;
        let s = format!("{} {}", "x".dim(), "yz".cyan().bold());
        assert_eq!(width(&s), 4);
        let l = to_line(&s);
        let text: String = l.spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "x yz");
    }

    #[test]
    fn wraps_wide_chars() {
        let l = to_line("日本語テキスト");
        let w = wrap(l, 6);
        assert_eq!(w.len(), 3);
        assert!(w.iter().all(|l| l.width() <= 6));
        assert_eq!(wrap(to_line(""), 10).len(), 1);
    }
}
