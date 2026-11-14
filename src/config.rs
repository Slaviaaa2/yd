//! Persistent settings: `<config_dir>/yd/config.toml` (override dir with `YD_CONFIG_DIR`).

#![allow(dead_code)] // public API also used by the interactive mode

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

const MAX_RECENT: usize = 10;

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

    /// Add (or replace, keeping position) a favorite. `path` is stored absolute.
    pub fn add_favorite(&mut self, name: &str, path: &str) -> Result<PathBuf> {
        let name = name.trim();
        if name.is_empty() || name.contains(['/', '\\', '~']) || name.starts_with('.') {
            bail!("invalid favorite name '{name}' (no slashes, no leading dot)");
        }
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
        self.recent.truncate(MAX_RECENT);
    }

    /// Resolve `-o` style input: favorite name, or a path (`~` expanded, relative made absolute).
    /// The directory is created if missing. A bare word that is no favorite is an error
    /// (use `./name` for a relative directory).
    pub fn resolve_dest(&self, s: &str) -> Result<PathBuf> {
        let s = s.trim();
        if s.is_empty() {
            bail!("empty destination");
        }
        let is_pathlike = s.contains(['/', '\\']) || s.starts_with(['~', '.']);
        let dir = if let Some(f) = self.favorite(s) {
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
}
