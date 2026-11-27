//! Syntax highlighting of the composer line (and the echoed `› …` line in the transcript).

use super::command;
use crate::config::Config;
use crate::format::FormatSpec;
use crate::ui;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Plain,
    Accent,
    Bad,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seg {
    pub text: String,
    pub kind: Kind,
}

fn seg(text: &str, kind: Kind) -> Seg {
    Seg {
        text: text.to_string(),
        kind,
    }
}

fn format_ok(val: &str, partial: bool) -> bool {
    FormatSpec::parse(val).is_ok()
        || (partial && {
            let v = val.to_ascii_lowercase();
            FormatSpec::all_choices().iter().any(|s| s.canonical().starts_with(&v))
        })
}

fn dest_ok(cfg: &Config, val: &str, partial: bool) -> bool {
    crate::config::is_temp_sh_dest(val) || cfg.dest_is_valid(val)
        || (partial && {
            let v = val.to_lowercase();
            crate::config::TEMP_SH_DEST.starts_with(&v) || v == "f" || v == "r" || v == "f:" || v == "r:" || cfg.favorites.iter().any(|f| f.name.to_lowercase().starts_with(&v))
        })
}

/// Split `text` into styled segments. The token at the very end is treated as "still being typed".
pub fn segments(text: &str, cfg: &Config) -> Vec<Seg> {
    // Tokens with byte ranges.
    let mut toks: Vec<(usize, usize)> = Vec::new();
    let mut st = None;
    for (i, c) in text.char_indices() {
        match (c.is_whitespace(), st) {
            (false, None) => st = Some(i),
            (true, Some(s)) => {
                toks.push((s, i));
                st = None;
            }
            _ => {}
        }
    }
    if let Some(s) = st {
        toks.push((s, text.len()));
    }
    let typing_last = st.is_some();
    let words: Vec<&str> = toks.iter().map(|&(s, e)| &text[s..e]).collect();
    let mut kinds = vec![Kind::Plain; words.len()];

    if command::has_url(&words) {
        let mut i = 0;
        while i < words.len() {
            let w = words[i];
            let lw = w.to_ascii_lowercase();
            let is_last = |j: usize| typing_last && j == words.len() - 1;
            if command::takes_arg(w) {
                kinds[i] = Kind::Accent;
                if let Some(v) = words.get(i + 1) {
                    let partial = is_last(i + 1);
                    let ok = if matches!(lw.as_str(), "/f" | "/format") {
                        format_ok(v, partial)
                    } else {
                        dest_ok(cfg, v, partial)
                    };
                    kinds[i + 1] = if ok { Kind::Accent } else { Kind::Bad };
                }
                i += 2;
                continue;
            }
            if lw == "/force" {
                kinds[i] = Kind::Accent;
            } else if w.starts_with('/') && !w.contains('.') {
                let prefix_of_mod = is_last(i) && ["/f", "/o", "/force", "/format", "/out"].iter().any(|m| m.starts_with(&lw));
                kinds[i] = if prefix_of_mod { Kind::Accent } else { Kind::Bad };
            } else if !command::looks_like_url(w) {
                kinds[i] = Kind::Bad;
            }
            i += 1;
        }
    } else if let Some(first) = words.first().filter(|w| w.starts_with('/')) {
        let name = &first[1..];
        let partial = typing_last && words.len() == 1;
        kinds[0] = if command::lookup(name).is_some() || (partial && !command::matching(name).is_empty()) {
            Kind::Accent
        } else {
            Kind::Bad
        };
    }

    let mut out = Vec::new();
    let mut pos = 0;
    for (k, &(s, e)) in kinds.iter().zip(&toks) {
        if s > pos {
            out.push(seg(&text[pos..s], Kind::Plain));
        }
        out.push(seg(&text[s..e], *k));
        pos = e;
    }
    if pos < text.len() {
        out.push(seg(&text[pos..], Kind::Plain));
    }
    out
}

/// Keep only the part of `segs` inside the byte range `[start, end)`.
pub fn clip(segs: &[Seg], start: usize, end: usize) -> Vec<Seg> {
    let mut out = Vec::new();
    let mut pos = 0;
    for s in segs {
        let (a, b) = (pos, pos + s.text.len());
        pos = b;
        let (lo, hi) = (a.max(start), b.min(end));
        if lo < hi {
            out.push(seg(&s.text[lo - a..hi - a], s.kind));
        }
    }
    out
}

/// ANSI string for the segments (`dim`: the echoed variant).
pub fn render(segs: &[Seg], dim: bool) -> String {
    segs.iter()
        .map(|s| match (s.kind, dim) {
            (Kind::Plain, false) => s.text.clone(),
            (Kind::Plain, true) => ui::dim(&s.text),
            (Kind::Accent, false) => ui::cyan(&s.text),
            (Kind::Accent, true) => ui::dim(ui::cyan(&s.text)),
            (Kind::Bad, false) => ui::red(&s.text),
            (Kind::Bad, true) => ui::dim(ui::red(&s.text)),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Favorite;
    use std::path::PathBuf;

    fn kinds(text: &str) -> Vec<(String, Kind)> {
        let mut cfg = Config::default();
        cfg.favorites.push(Favorite {
            name: "music".into(),
            path: PathBuf::from("/m"),
        });
        segments(text, &cfg)
            .into_iter()
            .filter(|s| !s.text.trim().is_empty())
            .map(|s| (s.text, s.kind))
            .collect()
    }

    #[test]
    fn url_line_modifiers() {
        let k = kinds("https://a.com/x /f ogg /o f:0 ");
        assert_eq!(k[0].1, Kind::Plain);
        assert!(k[1..].iter().all(|(_, k)| *k == Kind::Accent), "{k:?}");
        let k = kinds("https://a.com/x /f nope");
        assert_eq!((k[1].1, k[2].1), (Kind::Accent, Kind::Bad));
        let k = kinds("https://a.com/x /o f:9 /zzz /force");
        assert_eq!(k[2].1, Kind::Bad);
        assert_eq!((k[3].1, k[4].1), (Kind::Bad, Kind::Accent));
        // Still typing: prefixes are fine.
        let k = kinds("https://a.com/x /f mp");
        assert_eq!(k[2].1, Kind::Accent);
        let k = kinds("https://a.com/x /fo");
        assert_eq!(k[1].1, Kind::Accent);
        let k = kinds("/f ogg https://a.com/x");
        assert_eq!((k[0].1, k[1].1, k[2].1), (Kind::Accent, Kind::Accent, Kind::Plain));
        assert_eq!(kinds("https://a.com/x hello")[1].1, Kind::Bad);
    }

    #[test]
    fn command_lines() {
        assert_eq!(kinds("/f nope")[0].1, Kind::Accent);
        assert_eq!(kinds("/f nope")[1].1, Kind::Plain);
        assert_eq!(kinds("/zzz")[0].1, Kind::Bad);
        assert_eq!(kinds("/se")[0].1, Kind::Accent);
        assert_eq!(kinds("/sx ")[0].1, Kind::Bad);
        assert_eq!(kinds("hello")[0].1, Kind::Plain);
    }

    #[test]
    fn clip_and_render() {
        let cfg = Config::default();
        let s = segments("https://a.com/x /f ogg", &cfg);
        let joined: String = s.iter().map(|x| x.text.as_str()).collect();
        assert_eq!(joined, "https://a.com/x /f ogg");
        let c = clip(&s, 14, 20);
        assert_eq!(c.iter().map(|x| x.text.as_str()).collect::<String>(), &joined[14..20]);
    }
}
