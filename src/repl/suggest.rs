//! Candidate generation for the composer: command names, modifiers, formats, destinations.

use std::path::PathBuf;

use super::command;
use crate::config::Favorite;
use crate::format::FormatSpec;
use crate::ui::picker::{dir_names, format_detail};
use crate::ui::abbrev_home;

pub struct Ctx<'a> {
    pub favorites: &'a [Favorite],
    pub recent: &'a [PathBuf],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cand {
    /// Text that replaces the token being typed.
    pub insert: String,
    pub label: String,
    pub detail: String,
}

#[derive(Debug, Default)]
pub struct Suggestions {
    /// Byte offset of the token being typed.
    pub start: usize,
    pub items: Vec<Cand>,
}

fn cand(insert: impl Into<String>, label: impl Into<String>, detail: impl Into<String>) -> Cand {
    Cand {
        insert: insert.into(),
        label: label.into(),
        detail: detail.into(),
    }
}

fn token_start(text: &str) -> usize {
    match text.rfind(char::is_whitespace) {
        Some(i) => i + text[i..].chars().next().map_or(1, char::len_utf8),
        None => 0,
    }
}

/// Candidates for the token at the end of `text`.
pub fn suggest(text: &str, ctx: &Ctx) -> Suggestions {
    let start = token_start(text);
    let tok = &text[start..];
    let prev: Vec<&str> = text[..start].split_whitespace().collect();
    let last = prev.last().map(|p| p.to_ascii_lowercase());
    let items = match last.as_deref() {
        Some("/f" | "/format") => formats(tok),
        Some("/o" | "/out") => dests(tok, ctx),
        Some("rm") if prev.len() == 2 && prev[0].eq_ignore_ascii_case("/fav") => fav_names(tok, ctx),
        _ if tok.starts_with('/') => {
            if prev.is_empty() { commands(&tok[1..]) } else { modifiers(tok) }
        }
        _ => Vec::new(),
    };
    Suggestions { start, items }
}

fn commands(prefix: &str) -> Vec<Cand> {
    command::matching(prefix)
        .into_iter()
        .map(|c| {
            let sp = if c.args.is_empty() { "" } else { " " };
            cand(format!("/{}{sp}", c.name), c.label(), c.help)
        })
        .collect()
}

fn modifiers(tok: &str) -> Vec<Cand> {
    const MODS: [(&str, &str); 5] = [
        ("/f", "format for this line"),
        ("/o", "save folder for this line"),
        ("/force", "overwrite existing files"),
        ("/format", "same as /f"),
        ("/out", "same as /o"),
    ];
    let t = tok.to_ascii_lowercase();
    MODS.iter()
        .filter(|(m, _)| m.starts_with(&t) && (t.len() > 1 || !matches!(*m, "/format" | "/out")))
        .map(|(m, d)| cand(format!("{m} "), *m, *d))
        .collect()
}

fn formats(tok: &str) -> Vec<Cand> {
    let t = tok.to_ascii_lowercase();
    FormatSpec::all_choices()
        .into_iter()
        .filter(|s| s.canonical().starts_with(&t))
        .map(|s| cand(s.canonical(), s.canonical(), format_detail(&s)))
        .collect()
}

fn fav_names(tok: &str, ctx: &Ctx) -> Vec<Cand> {
    let t = tok.to_lowercase();
    ctx.favorites
        .iter()
        .enumerate()
        .filter(|(_, f)| f.name.to_lowercase().starts_with(&t))
        .map(|(i, f)| cand(f.name.clone(), format!("f:{i} {}", f.name), abbrev_home(&f.path)))
        .collect()
}

fn dests(tok: &str, ctx: &Ctx) -> Vec<Cand> {
    let lower = tok.to_lowercase();
    if lower.starts_with("f:") {
        return ctx
            .favorites
            .iter()
            .enumerate()
            .filter(|(i, _)| format!("f:{i}").starts_with(&lower))
            .map(|(i, f)| cand(format!("f:{i}"), format!("f:{i} {}", f.name), abbrev_home(&f.path)))
            .collect();
    }
    if lower.starts_with("r:") {
        return recents(&lower, ctx);
    }
    if tok.contains('/') || tok.starts_with(['~', '.']) {
        let (dir, names) = dir_names(tok);
        return names.into_iter().map(|n| cand(format!("{dir}{n}/"), format!("{n}/"), "folder")).collect();
    }
    let mut out = fav_names(tok, ctx);
    if tok.is_empty() {
        out.extend(recents("r:", ctx));
    }
    out
}

fn recents(prefix: &str, ctx: &Ctx) -> Vec<Cand> {
    ctx.recent
        .iter()
        .enumerate()
        .filter(|(i, _)| format!("r:{i}").starts_with(prefix))
        .map(|(i, p)| cand(format!("r:{i}"), format!("r:{i}"), abbrev_home(p)))
        .collect()
}

/// The token is already exactly what the candidate would insert (Enter should submit).
pub fn is_complete(token: &str, c: &Cand) -> bool {
    let ins = c.insert.trim_end();
    token == ins || (ins.ends_with('/') && token == ins.trim_end_matches('/'))
}

/// Dim inline completion for the selected candidate (case-exact prefix only).
pub fn ghost(text: &str, sug: &Suggestions, sel: usize) -> String {
    let tok = &text[sug.start..];
    match sug.items.get(sel) {
        Some(c) if !tok.is_empty() && c.insert.len() > tok.len() && c.insert.starts_with(tok) => {
            c.insert[tok.len()..].trim_end().to_string()
        }
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> (Vec<Favorite>, Vec<PathBuf>) {
        (
            vec![
                Favorite {
                    name: "music".into(),
                    path: PathBuf::from("/m/music"),
                },
                Favorite {
                    name: "clips".into(),
                    path: PathBuf::from("/m/clips"),
                },
            ],
            vec![PathBuf::from("/r/one"), PathBuf::from("/r/two")],
        )
    }

    fn run(text: &str) -> (Suggestions, String) {
        let (f, r) = ctx();
        let s = suggest(text, &Ctx { favorites: &f, recent: &r });
        let g = ghost(text, &s, 0);
        (s, g)
    }

    fn inserts(s: &Suggestions) -> Vec<&str> {
        s.items.iter().map(|c| c.insert.as_str()).collect()
    }

    #[test]
    fn commands_and_modifiers() {
        let (s, g) = run("/se");
        assert_eq!(inserts(&s), ["/settings"]);
        assert_eq!(g, "ttings");
        let (s, _) = run("/");
        assert_eq!(s.items.len(), 7);
        let (s, _) = run("https://a.com/x /");
        assert_eq!(inserts(&s), ["/f ", "/o ", "/force "]);
        let (s, g) = run("https://a.com/x /fo");
        assert_eq!(inserts(&s), ["/force ", "/format "]);
        assert_eq!(g, "rce");
        assert!(run("hello").0.items.is_empty());
    }

    #[test]
    fn format_values() {
        let (s, g) = run("/f mp");
        assert_eq!(s.items[0].insert, "mp3");
        assert_eq!(g, "3");
        assert!(s.items.iter().all(|c| c.insert.starts_with("mp")));
        assert!(s.items.iter().any(|c| c.insert == "mp3:320" && c.detail == "audio"));
        let (s, _) = run("https://a.com/x /f o");
        assert!(inserts(&s).contains(&"ogg") && inserts(&s).contains(&"opus"));
        assert_eq!(s.start, "https://a.com/x /f ".len());
        assert!(run("/f zzz").0.items.is_empty());
    }

    #[test]
    fn dest_values() {
        let (s, g) = run("/o mu");
        assert_eq!(inserts(&s), ["music"]);
        assert_eq!(g, "sic");
        assert_eq!(s.items[0].label, "f:0 music");
        let (s, g) = run("/o f:");
        assert_eq!(inserts(&s), ["f:0", "f:1"]);
        assert_eq!(g, "0");
        let (s, _) = run("/o r:1");
        assert_eq!(inserts(&s), ["r:1"]);
        let (s, _) = run("/o ");
        assert_eq!(inserts(&s), ["music", "clips", "r:0", "r:1"]);
        let (s, _) = run("/fav rm cl");
        assert_eq!(inserts(&s), ["clips"]);
    }

    #[test]
    fn directory_completion() {
        let base = std::env::temp_dir().join(format!("yd-suggest-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("Music")).unwrap();
        std::fs::create_dir_all(base.join("Movies")).unwrap();
        let b = base.display().to_string();
        let (s, g) = run(&format!("/o {b}/Mu"));
        assert_eq!(inserts(&s), [format!("{b}/Music/")]);
        assert_eq!(g, "sic/");
        let (s, _) = run(&format!("/o {b}/"));
        assert_eq!(s.items.len(), 2);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn completeness() {
        let c = cand("music", "m", "");
        assert!(is_complete("music", &c));
        assert!(!is_complete("mu", &c));
        let d = cand("/a/Music/", "Music/", "");
        assert!(is_complete("/a/Music", &d) && is_complete("/a/Music/", &d));
        assert!(is_complete("/f", &cand("/f ", "", "")));
    }
}
