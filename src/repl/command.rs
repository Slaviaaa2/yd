//! Parsing of composer input: slash commands, URLs, `/fav` arguments.

use std::path::Path;

#[derive(Debug)]
pub struct Cmd {
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    pub args: &'static str,
    pub help: &'static str,
}

pub const COMMANDS: [Cmd; 10] = [
    Cmd {
        name: "f",
        aliases: &["format"],
        args: "[spec]",
        help: "Choose output format (mp3, mp3:320, mp4:1080 ...)",
    },
    Cmd {
        name: "o",
        aliases: &["out"],
        args: "[fav|path|ex:tempsh]",
        help: "Choose where files are saved",
    },
    Cmd {
        name: "fav",
        aliases: &[],
        args: "[name | rm <name>]",
        help: "Save current folder as favorite / list favorites",
    },
    Cmd {
        name: "open",
        aliases: &[],
        args: "",
        help: "Open the save folder in the file manager",
    },
    Cmd {
        name: "settings",
        aliases: &["set"],
        args: "",
        help: "Open the settings screen",
    },
    Cmd {
        name: "update",
        aliases: &[],
        args: "",
        help: "Update helper tools and check for a new yd version",
    },
    Cmd {
        name: "upgrade",
        aliases: &[],
        args: "",
        help: "Install the latest published yd version",
    },
    Cmd {
        name: "history",
        aliases: &[],
        args: "",
        help: "View inputs and results from past sessions",
    },
    Cmd {
        name: "help",
        aliases: &[],
        args: "",
        help: "Show commands and keys",
    },
    Cmd {
        name: "q",
        aliases: &["quit", "exit"],
        args: "",
        help: "Quit",
    },
];

impl Cmd {
    fn is(&self, word: &str) -> bool {
        self.name == word || self.aliases.contains(&word)
    }

    /// `/f /format`-style label for popups and help.
    pub fn label(&self) -> String {
        let mut s = format!("/{}", self.name);
        for a in self.aliases {
            s.push_str(&format!(" /{a}"));
        }
        s
    }
}

pub fn lookup(word: &str) -> Option<&'static Cmd> {
    let w = word.to_ascii_lowercase();
    COMMANDS.iter().find(|c| c.is(&w))
}

/// Commands whose name or alias starts with `prefix` (without the slash).
pub fn matching(prefix: &str) -> Vec<&'static Cmd> {
    let p = prefix.to_ascii_lowercase();
    COMMANDS
        .iter()
        .filter(|c| c.name.starts_with(&p) || c.aliases.iter().any(|a| a.starts_with(&p)))
        .collect()
}

pub fn looks_like_url(tok: &str) -> bool {
    tok.contains("://") || tok.starts_with("www.") || (tok.contains('.') && tok.contains('/'))
}

/// All whitespace-separated tokens as URLs, or `None` if any token is not URL-like (or none exist).
pub fn extract_urls(text: &str) -> Option<Vec<String>> {
    let toks: Vec<&str> = text.split_whitespace().collect();
    if toks.is_empty() || !toks.iter().all(|t| looks_like_url(t)) {
        return None;
    }
    Some(toks.into_iter().map(String::from).collect())
}

/// URLs plus one-off modifiers from a single line.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Inline {
    pub urls: Vec<String>,
    pub format: Option<String>,
    pub dest: Option<String>,
    pub force: bool,
}

pub fn takes_arg(tok: &str) -> bool {
    matches!(tok.to_ascii_lowercase().as_str(), "/f" | "/format" | "/o" | "/out")
}

/// Does the line contain a URL outside of modifier arguments?
pub fn has_url(toks: &[&str]) -> bool {
    let mut i = 0;
    while i < toks.len() {
        if takes_arg(toks[i]) {
            i += 2;
            continue;
        }
        if looks_like_url(toks[i]) {
            return true;
        }
        i += 1;
    }
    false
}

/// Parse `URL... [/f spec] [/o dest] [/force]` in any order.
pub fn parse_inline(line: &str) -> Result<Inline, String> {
    let toks: Vec<&str> = line.split_whitespace().collect();
    let mut out = Inline::default();
    let mut i = 0;
    while i < toks.len() {
        let t = toks[i];
        let lower = t.to_ascii_lowercase();
        if takes_arg(t) {
            let Some(arg) = toks.get(i + 1) else {
                return Err(format!("{t} needs a value"));
            };
            let slot = if matches!(lower.as_str(), "/f" | "/format") { &mut out.format } else { &mut out.dest };
            if slot.is_some() {
                return Err(format!("{t} given twice"));
            }
            *slot = Some((*arg).to_string());
            i += 2;
        } else if lower == "/force" {
            out.force = true;
            i += 1;
        } else if t.starts_with('/') && !t.contains('.') {
            return Err(format!("unknown modifier {t} (use /f, /o, /force)"));
        } else if looks_like_url(t) {
            out.urls.push(t.to_string());
            i += 1;
        } else {
            return Err(format!("not a URL: {}", t));
        }
    }
    Ok(out)
}

#[derive(Debug)]
pub enum Parsed {
    Empty,
    Urls(Vec<String>),
    /// URLs with modifiers (`Err` = invalid line).
    Inline(Result<Inline, String>),
    Command { cmd: &'static Cmd, args: String },
    UnknownCommand(String),
    NotUrl(String),
}

pub fn parse_input(line: &str) -> Parsed {
    let line = line.trim();
    if line.is_empty() {
        return Parsed::Empty;
    }
    let toks: Vec<&str> = line.split_whitespace().collect();
    if has_url(&toks) {
        return match parse_inline(line) {
            Ok(i) if i.format.is_none() && i.dest.is_none() && !i.force => Parsed::Urls(i.urls),
            r => Parsed::Inline(r),
        };
    }
    if let Some(rest) = line.strip_prefix('/') {
        let (word, args) = match rest.split_once(char::is_whitespace) {
            Some((w, a)) => (w, a.trim()),
            None => (rest, ""),
        };
        return match lookup(word) {
            Some(cmd) => Parsed::Command {
                cmd,
                args: args.to_string(),
            },
            None => Parsed::UnknownCommand(word.to_string()),
        };
    }
    match extract_urls(line) {
        Some(u) => Parsed::Urls(u),
        None => {
            let bad = line.split_whitespace().find(|t| !looks_like_url(t)).unwrap_or(line);
            Parsed::NotUrl(bad.to_string())
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum FavArgs {
    List,
    Add(String),
    Rm(String),
    Bad(&'static str),
}

pub fn parse_fav(args: &str) -> FavArgs {
    let toks: Vec<&str> = args.split_whitespace().collect();
    match toks.as_slice() {
        [] => FavArgs::List,
        [rm, name] if matches!(*rm, "rm" | "remove" | "del") => FavArgs::Rm((*name).to_string()),
        [rm] if matches!(*rm, "rm" | "remove" | "del") => FavArgs::Bad("usage: /fav rm <name>"),
        [name] => FavArgs::Add((*name).to_string()),
        _ => FavArgs::Bad("usage: /fav <name> | /fav rm <name>"),
    }
}

/// Program and arguments that open `dir` in the system file manager.
pub fn open_command(dir: &Path) -> (&'static str, Vec<String>) {
    let program = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(target_os = "windows") {
        "explorer"
    } else {
        "xdg-open"
    };
    (program, vec![dir.display().to_string()])
}

/// Launch the file manager detached, with all stdio silenced.
pub fn open_folder(dir: &Path) -> std::io::Result<()> {
    use std::process::{Command, Stdio};
    let (program, args) = open_command(dir);
    let mut cmd = Command::new(program);
    cmd.args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let mut child = cmd.spawn()?;
    // Reap it in the background so no zombie is left behind.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_splitting() {
        let t = "https://a.com/x\nhttps://b.com/y  https://c.com/z\r\n";
        assert_eq!(extract_urls(t).unwrap().len(), 3);
        assert!(extract_urls("https://a.com/x hello").is_none());
        assert!(extract_urls("   ").is_none());
        assert!(looks_like_url("youtu.be/abc"));
        assert!(looks_like_url("www.example.com"));
        assert!(!looks_like_url("hello"));
    }

    #[test]
    fn parses_commands() {
        assert!(matches!(parse_input("  "), Parsed::Empty));
        match parse_input("/format mp3:320") {
            Parsed::Command { cmd, args } => {
                assert_eq!(cmd.name, "f");
                assert_eq!(args, "mp3:320");
            }
            p => panic!("{p:?}"),
        }
        assert!(matches!(parse_input("/Q"), Parsed::Command { cmd, .. } if cmd.name == "q"));
        assert!(matches!(parse_input("/exit"), Parsed::Command { cmd, .. } if cmd.name == "q"));
        assert!(matches!(parse_input("/zzz a b"), Parsed::UnknownCommand(w) if w == "zzz"));
        assert!(matches!(parse_input("https://x.com/a https://x.com/b"), Parsed::Urls(u) if u.len() == 2));
        assert!(matches!(parse_input("hello world"), Parsed::NotUrl(w) if w == "hello"));
    }

    #[test]
    fn inline_modifiers() {
        let p = |l: &str| parse_inline(l);
        let r = p("https://youtu.be/x /f ogg /o f:0").unwrap();
        assert_eq!(r.urls, ["https://youtu.be/x"]);
        assert_eq!((r.format.as_deref(), r.dest.as_deref(), r.force), (Some("ogg"), Some("f:0"), false));
        let r = p("/f mp4:720 /o ~/clips https://a.com/1 https://a.com/2").unwrap();
        assert_eq!(r.urls.len(), 2);
        assert_eq!((r.format.as_deref(), r.dest.as_deref()), (Some("mp4:720"), Some("~/clips")));
        let r = p("https://a.com/1 /FORCE /out /mnt/data.x/y").unwrap();
        assert!(r.force);
        assert_eq!(r.dest.as_deref(), Some("/mnt/data.x/y"));
        assert!(p("https://a.com/1 /f").is_err());
        assert!(p("https://a.com/1 /f a /format b").is_err());
        assert!(p("https://a.com/1 /zzz").is_err());
        assert!(p("https://a.com/1 hello").is_err());
    }

    #[test]
    fn inline_detection() {
        assert!(matches!(parse_input("https://a.com/1 /f ogg"), Parsed::Inline(Ok(_))));
        assert!(matches!(parse_input("/f ogg https://a.com/1"), Parsed::Inline(Ok(_))));
        assert!(matches!(parse_input("https://a.com/1 /zzz"), Parsed::Inline(Err(_))));
        // No URL: the normal commands, even when an argument looks path-like.
        assert!(matches!(parse_input("/f ogg"), Parsed::Command { .. }));
        assert!(matches!(parse_input("/o /mnt/a.b/c"), Parsed::Command { .. }));
        assert!(matches!(parse_input("https://a.com/1"), Parsed::Urls(_)));
    }

    #[test]
    fn command_matching() {
        let names = |p: &str| matching(p).iter().map(|c| c.name).collect::<Vec<_>>();
        assert_eq!(names(""), ["f", "o", "fav", "open", "settings", "update", "upgrade", "history", "help", "q"]);
        assert_eq!(names("se"), ["settings"]);
        assert_eq!(names("f"), ["f", "fav"]);
        assert_eq!(names("fo"), ["f"]);
        assert_eq!(names("ex"), ["q"]);
        assert!(names("x").is_empty());
        assert_eq!(lookup("quit").unwrap().label(), "/q /quit /exit");
    }

    #[test]
    fn fav_args() {
        assert_eq!(parse_fav(""), FavArgs::List);
        assert_eq!(parse_fav("music"), FavArgs::Add("music".into()));
        assert_eq!(parse_fav("rm music"), FavArgs::Rm("music".into()));
        assert!(matches!(parse_fav("rm"), FavArgs::Bad(_)));
        assert!(matches!(parse_fav("a b"), FavArgs::Bad(_)));
    }

    #[test]
    fn open_builds_command() {
        let (prog, args) = open_command(Path::new("/tmp/x y"));
        if cfg!(target_os = "linux") {
            assert_eq!(prog, "xdg-open");
        }
        assert_eq!(args, ["/tmp/x y"]);
    }
}
