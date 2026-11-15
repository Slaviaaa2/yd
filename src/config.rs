//! Persistent settings: `<config_dir>/yd/config.toml` (override dir with `YD_CONFIG_DIR`).

#![allow(dead_code)] // public API also used by the interactive mode

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

pub const DEFAULT_RECENT_KEEP: usize = 10;
pub const DEFAULT_HISTORY_KEEP: usize = 500;

/// Interactive-mode settings (`[settings]` in config.toml). Missing keys = defaults.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Settings {
    pub recent_keep: usize,
    pub history_persist: bool,
    pub history_keep: usize,
    /// Overwrite existing files by default.
    pub force: bool,
    /// Download the whole playlist for v=&list= URLs by default.
    pub playlist: bool,
    pub cookies: Option<String>,
    /// Print the session summary after leaving the alternate screen.
    pub exit_summary: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            recent_keep: DEFAULT_RECENT_KEEP,
            history_persist: true,
            history_keep: DEFAULT_HISTORY_KEEP,
            force: false,
            playlist: false,
            cookies: None,
            exit_summary: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Favorite {
    pub name: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub favorites: Vec<Favorite>,
    /// Most recent first, deduplicated, at most 10.
    pub recent: Vec<PathBuf>,
    pub last_format: Option<String>,
    pub last_dest: Option<PathBuf>,
    pub template: Option<String>,
    pub settings: Settings,
}

pub fn config_dir() -> PathBuf {
    match std::env::var_os("YD_CONFIG_DIR") {
        Some(d) if !d.is_empty() => PathBuf::from(d),
        _ => dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")).join("yd"),
    }
}

pub fn config_path() -> PathBuf {
    config_dir().join("config.toml")
}

/// Input history file (one entry per line).
pub fn history_path() -> PathBuf {
    config_dir().join("history")
}

/// Read the history file; missing = empty.
pub fn load_history() -> Vec<String> {
    std::fs::read_to_string(history_path())
        .map(|t| t.lines().filter(|l| !l.trim().is_empty()).map(String::from).collect())
        .unwrap_or_default()
}

pub fn save_history(items: &[String]) -> Result<()> {
    let path = history_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut text = items.join("\n");
    if !text.is_empty() {
        text.push('\n');
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// Expand a leading `~` / `~/`.
pub fn expand_tilde(s: &str) -> PathBuf {
    if s == "~" {
        return dirs::home_dir().unwrap_or_else(|| PathBuf::from("~"));
    }
    if let Some(rest) = s.strip_prefix("~/")
        && let Some(home) = dirs::home_dir()
    {
        return home.join(rest);
    }
    PathBuf::from(s)
}

/// Expand `~` and make absolute (relative to cwd) without touching the filesystem.
pub fn absolutize(s: &str) -> Result<PathBuf> {
    let p = expand_tilde(s);
    std::path::absolute(&p).with_context(|| format!("cannot resolve path '{s}'"))
}

impl Config {
    /// Load the config. Missing => defaults. Corrupt => defaults, file is moved to `config.toml.bak`
    /// and a warning message is returned.
    pub fn load() -> (Config, Option<String>) {
        let path = config_path();
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (Config::default(), None),
            Err(e) => return (Config::default(), Some(format!("cannot read {}: {e}", path.display()))),
        };
        match toml::from_str::<Config>(&text) {
            Ok(c) => (c, None),
            Err(e) => {
                let bak = path.with_extension("toml.bak");
                let moved = std::fs::rename(&path, &bak).is_ok();
                let msg = if moved {
                    format!(
                        "config is corrupt ({}); using defaults, old file saved as {}",
                        e.message(),
                        bak.display()
                    )
                } else {
                    format!("config is corrupt ({}); using defaults", e.message())
                };
                (Config::default(), Some(msg))
            }
        }
    }

    /// Atomically write the config (tmp file + rename).
    pub fn save(&self) -> Result<()> {
        let path = config_path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
        }
        let text = toml::to_string_pretty(self).context("cannot serialize config")?;
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, text).with_context(|| format!("cannot write {}", tmp.display()))?;
        std::fs::rename(&tmp, &path).with_context(|| format!("cannot write {}", path.display()))?;
        Ok(())
    }

    pub fn favorite(&self, name: &str) -> Option<&Favorite> {
        self.favorites.iter().find(|f| f.name.eq_ignore_ascii_case(name))
    }

    fn check_name(name: &str) -> Result<()> {
        if name.is_empty() || name.contains(['/', '\\', '~', ':']) || name.starts_with('.') || name.contains(char::is_whitespace) {
            bail!("invalid favorite name '{name}' (no slashes, colons or spaces, no leading dot)");
        }
        Ok(())
    }

    /// Rename favorite `idx`; fails on an invalid name or one used by another favorite.
    pub fn rename_favorite(&mut self, idx: usize, new: &str) -> Result<()> {
        let new = new.trim();
        Self::check_name(new)?;
        if self.favorites.iter().enumerate().any(|(i, f)| i != idx && f.name.eq_ignore_ascii_case(new)) {
            bail!("a favorite named '{new}' already exists");
        }
        match self.favorites.get_mut(idx) {
            Some(f) => f.name = new.to_string(),
            None => bail!("no favorite f:{idx}"),
        }
        Ok(())
    }

    /// Cheap check used for live highlighting: would `resolve_dest` accept `s` (ignoring the filesystem)?
    pub fn dest_is_valid(&self, s: &str) -> bool {
        match self.resolve_index(s) {
            Some(r) => r.is_ok(),
            None => self.favorite(s).is_some() || s.contains(['/', '\\']) || s.starts_with(['~', '.']),
        }
    }

    /// Add (or replace, keeping position) a favorite. `path` is stored absolute.
    pub fn add_favorite(&mut self, name: &str, path: &str) -> Result<PathBuf> {
        let name = name.trim();
        Self::check_name(name)?;
        let path = absolutize(path)?;
        if let Some(f) = self.favorites.iter_mut().find(|f| f.name.eq_ignore_ascii_case(name)) {
            f.name = name.to_string();
            f.path = path.clone();
        } else {
            self.favorites.push(Favorite {
                name: name.to_string(),
                path: path.clone(),
            });
        }
        Ok(path)
    }

    /// Remove a favorite; false if it did not exist.
    pub fn remove_favorite(&mut self, name: &str) -> bool {
        let before = self.favorites.len();
        self.favorites.retain(|f| !f.name.eq_ignore_ascii_case(name));
        self.favorites.len() != before
    }

    /// Record a destination: moves to the front, dedups, keeps 10.
    pub fn push_recent(&mut self, dir: &Path) {
        self.recent.retain(|p| p != dir);
        self.recent.insert(0, dir.to_path_buf());
        self.recent.truncate(self.settings.recent_keep.max(1));
    }

    /// `f:N` / `r:N` (0-based index into favorites / recent as listed). `None` if `s` is not that form.
    fn resolve_index(&self, s: &str) -> Option<Result<PathBuf>> {
        let (kind, n) = s.split_once(':')?;
        if !matches!(kind, "f" | "r") || n.is_empty() || !n.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let n: usize = n.parse().unwrap_or(usize::MAX);
        let (what, len, found) = if kind == "f" {
            ("favorite", self.favorites.len(), self.favorites.get(n).map(|f| f.path.clone()))
        } else {
            ("recent folder", self.recent.len(), self.recent.get(n).cloned())
        };
        Some(found.ok_or_else(|| {
            if len == 0 {
                anyhow::anyhow!("no {what} {s}: none saved yet")
            } else if len == 1 {
                anyhow::anyhow!("no {what} {s}: 1 saved ({kind}:0)")
            } else {
                anyhow::anyhow!("no {what} {s}: {len} saved ({kind}:0..{kind}:{})", len - 1)
            }
        }))
    }

    /// Resolve `-o` style input: `f:N` / `r:N`, favorite name, or a path (`~` expanded, relative made absolute).
    /// The directory is created if missing. A bare word that is no favorite is an error
    /// (use `./name` for a relative directory).
    pub fn resolve_dest(&self, s: &str) -> Result<PathBuf> {
        let s = s.trim();
        if s.is_empty() {
            bail!("empty destination");
        }
        let is_pathlike = s.contains(['/', '\\']) || s.starts_with(['~', '.']);
        let dir = if let Some(r) = self.resolve_index(s) {
            r?
        } else if let Some(f) = self.favorite(s) {
            f.path.clone()
        } else if is_pathlike {
            absolutize(s)?
        } else {
            bail!("'{s}' is not a favorite (use ./{s} for a directory, or `yd fav add {s}`)");
        };
        std::fs::create_dir_all(&dir).with_context(|| format!("cannot create {}", dir.display()))?;
        Ok(dir)
    }

    /// Destination when `-o` is not given: last used, else Downloads, else cwd.
    pub fn default_dest(&self) -> PathBuf {
        self.last_dest
            .clone()
            .or_else(dirs::download_dir)
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."))
    }

    /// Record a finished download.
    pub fn record_download(&mut self, dest: &Path, format: &str) {
        self.push_recent(dest);
        self.last_dest = Some(dest.to_path_buf());
        self.last_format = Some(format.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recent_dedup_and_cap() {
        let mut c = Config::default();
        for i in 0..15 {
            c.push_recent(Path::new(&format!("/d/{i}")));
        }
        c.push_recent(Path::new("/d/10"));
        assert_eq!(c.recent.len(), 10);
        assert_eq!(c.recent[0], Path::new("/d/10"));
        assert_eq!(c.recent.iter().filter(|p| **p == Path::new("/d/10")).count(), 1);
    }

    #[test]
    fn favorites_add_replace_remove() {
        let mut c = Config::default();
        c.add_favorite("music", "/a").unwrap();
        c.add_favorite("vid", "/b").unwrap();
        c.add_favorite("Music", "/c").unwrap();
        assert_eq!(c.favorites.len(), 2);
        assert_eq!(c.favorites[0].path, Path::new("/c"));
        assert!(c.remove_favorite("VID"));
        assert!(!c.remove_favorite("vid"));
        assert!(c.add_favorite("a/b", "/x").is_err());
    }

    #[test]
    fn old_config_without_settings() {
        let old = "last_format = \"mp3\"\nrecent = [\"/a\"]\n[[favorites]]\nname = \"m\"\npath = \"/m\"\n";
        let c: Config = toml::from_str(old).unwrap();
        assert_eq!(c.settings, Settings::default());
        assert_eq!(c.favorites.len(), 1);
        let c: Config = toml::from_str("[settings]\nrecent_keep = 5\ncookies = \"firefox\"\n").unwrap();
        assert_eq!(c.settings.recent_keep, 5);
        assert_eq!(c.settings.cookies.as_deref(), Some("firefox"));
        assert!(c.settings.history_persist && c.settings.exit_summary);
        let t = toml::to_string_pretty(&c).unwrap();
        assert_eq!(toml::from_str::<Config>(&t).unwrap().settings, c.settings);
    }

    #[test]
    fn recent_cap_follows_settings_and_rename() {
        let mut c = Config::default();
        c.settings.recent_keep = 3;
        for i in 0..6 {
            c.push_recent(Path::new(&format!("/d/{i}")));
        }
        assert_eq!(c.recent.len(), 3);
        c.add_favorite("a", "/x").unwrap();
        c.add_favorite("b", "/y").unwrap();
        assert!(c.rename_favorite(0, "b").is_err());
        assert!(c.rename_favorite(0, "bad name").is_err());
        c.rename_favorite(0, "z").unwrap();
        assert_eq!(c.favorites[0].name, "z");
        assert!(c.dest_is_valid("f:1") && !c.dest_is_valid("f:5") && c.dest_is_valid("~/x") && !c.dest_is_valid("nope"));
    }

    #[test]
    fn roundtrip_toml() {
        let mut c = Config::default();
        c.add_favorite("m", "/music").unwrap();
        c.record_download(Path::new("/music"), "mp3:320");
        let t = toml::to_string_pretty(&c).unwrap();
        let d: Config = toml::from_str(&t).unwrap();
        assert_eq!(d.favorites, c.favorites);
        assert_eq!(d.last_format.as_deref(), Some("mp3:320"));
        assert!(toml::from_str::<Config>("").unwrap().favorites.is_empty());
    }

    #[test]
    fn dest_resolution() {
        let mut c = Config::default();
        c.favorites.push(Favorite {
            name: "m".into(),
            path: PathBuf::from("/tmp"),
        });
        assert_eq!(c.resolve_dest("m").unwrap(), Path::new("/tmp"));
        assert!(c.resolve_dest("nope").is_err());
    }

    #[test]
    fn index_references() {
        let mut c = Config::default();
        c.favorites.push(Favorite {
            name: "m".into(),
            path: PathBuf::from("/tmp"),
        });
        c.recent = vec![PathBuf::from("/var"), PathBuf::from("/tmp")];
        assert_eq!(c.resolve_dest("f:0").unwrap(), Path::new("/tmp"));
        assert_eq!(c.resolve_dest("r:0").unwrap(), Path::new("/var"));
        assert_eq!(c.resolve_dest("r:1").unwrap(), Path::new("/tmp"));
        let e = c.resolve_dest("f:9").unwrap_err().to_string();
        assert!(e.contains("1 saved"), "{e}");
        assert!(c.resolve_dest("r:2").is_err());
        assert!(Config::default().resolve_dest("f:0").unwrap_err().to_string().contains("none"));
        // Not index forms: treated as names / errors, never as indices.
        assert!(c.resolve_dest("f0").is_err());
        assert!(c.resolve_dest("f:x").is_err());
        assert!(c.add_favorite("f:0", "/x").is_err());
    }
}
