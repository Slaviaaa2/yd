//! `/fav` manager and `/settings` screen: modals that edit the config directly.

use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use unicode_width::UnicodeWidthStr;

use super::{App, Modal};
use crate::config::{Favorite, history_path, save_history};
use crate::ui::input::LineEditor;
use crate::ui::picker::{DestPicker, FormatPicker};
use crate::ui::{self, bold, cyan, detail, dim, green, heading, red, truncate};

type Out = (Vec<String>, Option<(u16, u16)>);

fn ctrl_c(k: &KeyEvent) -> bool {
    k.code == KeyCode::Char('c') && k.modifiers.contains(KeyModifiers::CONTROL)
}

fn status_line(s: &Option<(bool, String)>, width: usize) -> Option<String> {
    s.as_ref().map(|(ok, m)| {
        let m = truncate(m, width.saturating_sub(6));
        if *ok { format!("  {} {m}", green(ui::CHECK)) } else { format!("  {} {m}", red(ui::CROSS)) }
    })
}

/// Rows shown at once (the modal replaces the composer; leave room for title/status/footer).
fn visible_rows(height: usize, total: usize) -> usize {
    total.min(height.saturating_sub(5).max(4)).max(1)
}

fn window_top(sel: usize, vis: usize) -> usize {
    if sel < vis { 0 } else { sel + 1 - vis }
}

// ---------------------------------------------------------------------------------------------
// /fav
// ---------------------------------------------------------------------------------------------

enum PromptKind {
    Add,
    Rename(usize),
    Pin(PathBuf),
}

struct Prompt {
    kind: PromptKind,
    ed: LineEditor,
}

enum Row {
    Fav(usize),
    Recent(usize),
}

#[derive(Default)]
pub struct FavState {
    sel: usize,
    prompt: Option<Prompt>,
    status: Option<(bool, String)>,
    undo: Option<(usize, Favorite)>,
    added: usize,
    removed: usize,
    renamed: usize,
}

fn suggest_name(p: &std::path::Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default()
        .chars()
        .filter(|c| c.is_alphanumeric() || matches!(c, '-' | '_'))
        .collect()
}

impl App {
    fn fav_rows(&self) -> Vec<Row> {
        (0..self.cfg.favorites.len())
            .map(Row::Fav)
            .chain((0..self.cfg.recent.len()).map(Row::Recent))
            .collect()
    }

    fn row_path(&self, r: &Row) -> PathBuf {
        match r {
            Row::Fav(i) => self.cfg.favorites[*i].path.clone(),
            Row::Recent(i) => self.cfg.recent[*i].clone(),
        }
    }

    pub(super) fn open_fav(&mut self) {
        self.modal = Some(Modal::Fav(FavState::default()));
    }

    pub(super) fn fav_lines(&self, st: &FavState, width: usize) -> Out {
        let rows = self.fav_rows();
        let vis = visible_rows(self.height, rows.len());
        let top = window_top(st.sel, vis);
        let mut out = vec![format!("  {}", bold("Favorites & recent"))];
        if rows.is_empty() {
            out.push(dim("    nothing saved yet"));
        }
        let label_w = rows
            .iter()
            .map(|r| match r {
                Row::Fav(i) => format!("f:{i} \u{2605} {}", self.cfg.favorites[*i].name).width(),
                Row::Recent(i) => format!("r:{i} \u{b7}").width(),
            })
            .max()
            .unwrap_or(0);
        for (n, r) in rows.iter().enumerate().skip(top).take(vis) {
            let (label, path) = match r {
                Row::Fav(i) => (format!("f:{i} \u{2605} {}", self.cfg.favorites[*i].name), ui::abbrev_home(&self.cfg.favorites[*i].path)),
                Row::Recent(i) => (format!("r:{i} \u{b7}"), ui::abbrev_home(&self.cfg.recent[*i])),
            };
            let pad = " ".repeat(label_w.saturating_sub(label.width()));
            let path = truncate(&path, width.saturating_sub(label_w + 8));
            let cur = if self.cfg.last_dest.as_ref() == Some(&self.dest) && self.row_path(r) == self.dest { dim("  (current)") } else { String::new() };
            if n == st.sel {
                out.push(format!("  {} {}{pad}  {}{cur}", cyan("\u{203a}"), cyan(&label), cyan(&path)));
            } else {
                out.push(format!("    {label}{pad}  {}{cur}", dim(&path)));
            }
        }
        let mut cursor = None;
        if let Some(p) = &st.prompt {
            let what = match p.kind {
                PromptKind::Add => "Name for the current folder",
                PromptKind::Rename(_) => "New name",
                PromptKind::Pin(_) => "Favorite name",
            };
            let (shown, col) = p.ed.window(width.saturating_sub(what.width() + 6));
            out.push(format!("  {}: {shown}", bold(what)));
            cursor = Some(((what.width() + 4 + col) as u16, (out.len() - 1) as u16));
        }
        if let Some(s) = status_line(&st.status, width) {
            out.push(s);
        }
        let foot = if st.prompt.is_some() {
            "  \u{23ce} confirm \u{b7} esc cancel".to_string()
        } else {
            let mut parts = Vec::new();
            match rows.get(st.sel) {
                Some(Row::Fav(_)) => parts.extend(["\u{23ce} use", "a add current", "n rename", "d delete"]),
                Some(Row::Recent(_)) => parts.extend(["\u{23ce} use", "a add current", "p pin \u{2192} favorite"]),
                None => parts.push("a add current"),
            }
            if st.undo.is_some() {
                parts.push("u undo delete");
            }
            parts.push("esc close");
            format!("  {}", parts.join(" \u{b7} "))
        };
        out.push(dim(truncate(&foot, width.saturating_sub(1))));
        (out, cursor)
    }

    pub(super) fn fav_key(&mut self, mut st: FavState, k: KeyEvent) {
        if let Some(mut p) = st.prompt.take() {
            match k.code {
                KeyCode::Esc => {}
                KeyCode::Enter => {
                    let name = p.ed.text().trim().to_string();
                    match self.fav_apply_prompt(&mut st, &p.kind, &name) {
                        Ok(()) => {}
                        Err(e) => {
                            st.status = Some((false, e));
                            st.prompt = Some(p);
                        }
                    }
                }
                _ if ctrl_c(&k) => {}
                _ => {
                    p.ed.handle_key(&k);
                    st.prompt = Some(p);
                }
            }
            self.modal = Some(Modal::Fav(st));
            return;
        }
        let rows = self.fav_rows();
        let n = rows.len();
        st.sel = st.sel.min(n.saturating_sub(1));
        match k.code {
            KeyCode::Esc => return self.close_fav(st),
            _ if ctrl_c(&k) => return self.close_fav(st),
            KeyCode::Up if n > 0 => st.sel = (st.sel + n - 1) % n,
            KeyCode::Down if n > 0 => st.sel = (st.sel + 1) % n,
            KeyCode::Enter if n > 0 => {
                let path = self.row_path(&rows[st.sel]);
                st.status = Some(match self.cfg.resolve_dest(&path.display().to_string()) {
                    Ok(d) => {
                        let d = self.apply_dest(d);
                        (true, format!("Save to {}", ui::abbrev_home(&d)))
                    }
                    Err(e) => (false, format!("{e:#}")),
                });
            }
            KeyCode::Char('a') => {
                let mut ed = LineEditor::default();
                ed.set(&suggest_name(&self.dest));
                st.prompt = Some(Prompt { kind: PromptKind::Add, ed });
                st.status = None;
            }
            KeyCode::Char('n') => {
                if let Some(Row::Fav(i)) = rows.get(st.sel) {
                    let mut ed = LineEditor::default();
                    ed.set(&self.cfg.favorites[*i].name);
                    st.prompt = Some(Prompt { kind: PromptKind::Rename(*i), ed });
                    st.status = None;
                }
            }
            KeyCode::Char('p') => {
                if let Some(Row::Recent(i)) = rows.get(st.sel) {
                    let path = self.cfg.recent[*i].clone();
                    let mut ed = LineEditor::default();
                    ed.set(&suggest_name(&path));
                    st.prompt = Some(Prompt { kind: PromptKind::Pin(path), ed });
                    st.status = None;
                }
            }
            KeyCode::Char('d') => {
                if let Some(Row::Fav(i)) = rows.get(st.sel) {
                    let fav = self.cfg.favorites.remove(*i);
                    st.status = Some((true, format!("Deleted {} (u to undo)", fav.name)));
                    st.undo = Some((*i, fav));
                    st.removed += 1;
                    self.save_cfg();
                }
            }
            KeyCode::Char('u') => {
                if let Some((i, fav)) = st.undo.take() {
                    let i = i.min(self.cfg.favorites.len());
                    st.status = Some((true, format!("Restored {}", fav.name)));
                    self.cfg.favorites.insert(i, fav);
                    st.removed = st.removed.saturating_sub(1);
                    st.sel = i;
                    self.save_cfg();
                }
            }
            _ => {}
        }
        self.modal = Some(Modal::Fav(st));
    }

    fn fav_apply_prompt(&mut self, st: &mut FavState, kind: &PromptKind, name: &str) -> Result<(), String> {
        let exists = |app: &App| app.cfg.favorite(name).is_some();
        match kind {
            PromptKind::Add | PromptKind::Pin(_) => {
                if exists(self) {
                    return Err(format!("a favorite named '{name}' already exists"));
                }
                let path = match kind {
                    PromptKind::Pin(p) => p.clone(),
                    _ => self.dest.clone(),
                };
                self.cfg.add_favorite(name, &path.display().to_string()).map_err(|e| format!("{e:#}"))?;
                st.added += 1;
                st.sel = self.cfg.favorites.len() - 1;
                st.status = Some((true, format!("Added favorite {name}")));
            }
            PromptKind::Rename(i) => {
                self.cfg.rename_favorite(*i, name).map_err(|e| format!("{e:#}"))?;
                st.renamed += 1;
                st.status = Some((true, format!("Renamed to {name}")));
            }
        }
        self.save_cfg();
        Ok(())
    }

    fn close_fav(&mut self, st: FavState) {
        let mut parts = Vec::new();
        if st.added > 0 {
            parts.push(format!("+{}", st.added));
        }
        if st.removed > 0 {
            parts.push(format!("\u{2212}{}", st.removed));
        }
        if st.renamed > 0 {
            parts.push(format!("\u{270e}{}", st.renamed));
        }
        if !parts.is_empty() {
            self.push(heading(&format!("Favorites updated \u{b7} {}", parts.join(" "))));
        }
        self.modal = None;
    }
}

// ---------------------------------------------------------------------------------------------
// /settings
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Item {
    DefFormat,
    DefDest,
    AskSaveMethod,
    RecentKeep,
    ClearRecent,
    HistPersist,
    HistKeep,
    HistTrim,
    HistClear,
    Template,
    Force,
    Playlist,
    Cookies,
    ExitSummary,
}

const ITEMS: [Item; 14] = [
    Item::DefFormat,
    Item::DefDest,
    Item::AskSaveMethod,
    Item::RecentKeep,
    Item::ClearRecent,
    Item::HistPersist,
    Item::HistKeep,
    Item::HistTrim,
    Item::HistClear,
    Item::Template,
    Item::Force,
    Item::Playlist,
    Item::Cookies,
    Item::ExitSummary,
];

const RECENT_KEEP: [usize; 4] = [5, 10, 20, 50];
const HISTORY_KEEP: [usize; 4] = [100, 500, 1000, 5000];
const BROWSERS: [&str; 8] = ["firefox", "chrome", "chromium", "brave", "edge", "safari", "vivaldi", "opera"];

#[derive(Clone, Copy, PartialEq, Eq)]
enum EditKind {
    Template,
    TrimHistory,
}

#[derive(Default)]
pub struct SettingsState {
    sel: usize,
    edit: Option<(EditKind, LineEditor)>,
    status: Option<(bool, String)>,
    changes: usize,
}

impl Item {
    fn label(self) -> &'static str {
        match self {
            Item::DefFormat => "Default format",
            Item::DefDest => "Default save folder",
            Item::AskSaveMethod => "Ask where to save",
            Item::RecentKeep => "Recent folders \u{b7} keep",
            Item::ClearRecent => "Clear recent folders",
            Item::HistPersist => "History \u{b7} persist",
            Item::HistKeep => "History \u{b7} keep",
            Item::HistTrim => "Delete history older than\u{2026}",
            Item::HistClear => "Clear input history",
            Item::Template => "Filename template",
            Item::Force => "Overwrite existing",
            Item::Playlist => "Whole playlist (v=&list=)",
            Item::Cookies => "Cookies from browser",
            Item::ExitSummary => "Exit summary",
        }
    }
}

fn cycle<T: PartialEq + Copy>(opts: &[T], cur: T, dir: i32) -> T {
    let i = opts.iter().position(|o| *o == cur).unwrap_or(0) as i32;
    opts[(i + dir).rem_euclid(opts.len() as i32) as usize]
}

fn onoff(b: bool) -> String {
    if b { cyan("on") } else { dim("off") }
}

impl App {
    pub(super) fn open_settings(&mut self) {
        self.modal = Some(Modal::Settings(SettingsState::default()));
    }

    fn setting_value(&self, it: Item) -> String {
        let s = &self.cfg.settings;
        match it {
            Item::DefFormat => self.spec.to_string(),
            Item::DefDest => ui::abbrev_home(&self.dest),
            Item::AskSaveMethod => onoff(s.ask_save_method),
            Item::RecentKeep => format!("\u{2039} {} \u{203a}", s.recent_keep),
            Item::HistKeep => format!("\u{2039} {} \u{203a}", s.history_keep),
            Item::HistPersist => onoff(s.history_persist),
            Item::Template => match &self.cfg.template {
                Some(t) => t.clone(),
                None => dim("default"),
            },
            Item::Force => onoff(s.force),
            Item::Playlist => onoff(s.playlist),
            Item::Cookies => format!("\u{2039} {} \u{203a}", s.cookies.as_deref().unwrap_or("none")),
            Item::ExitSummary => onoff(s.exit_summary),
            Item::ClearRecent | Item::HistClear => String::new(),
            Item::HistTrim => dim("keep only the newest N"),
        }
    }

    pub(super) fn settings_lines(&self, st: &SettingsState, width: usize) -> Out {
        let vis = visible_rows(self.height, ITEMS.len());
        let top = window_top(st.sel, vis);
        let mut out = vec![format!("  {}", bold("Settings"))];
        let label_w = 30;
        let mut cursor = None;
        for (n, it) in ITEMS.iter().enumerate().skip(top).take(vis) {
            let mut value = self.setting_value(*it);
            let editing = st.edit.as_ref().filter(|_| n == st.sel);
            if let Some((_, ed)) = editing {
                let (shown, col) = ed.window(width.saturating_sub(label_w + 6));
                value = shown;
                cursor = Some(((4 + label_w + col) as u16, out.len() as u16));
            } else if ui::ansi::width(&value) > width.saturating_sub(label_w + 6) {
                value = truncate(&value, width.saturating_sub(label_w + 6));
            }
            let label = it.label();
            let pad = " ".repeat(label_w.saturating_sub(label.width()));
            if n == st.sel {
                out.push(format!("  {} {}{pad}{value}", cyan("\u{203a}"), cyan(label)));
            } else {
                out.push(format!("    {label}{pad}{value}"));
            }
        }
        if let Some(s) = status_line(&st.status, width) {
            out.push(s);
        }
        let it = ITEMS[st.sel.min(ITEMS.len() - 1)];
        let foot = if st.edit.is_some() {
            "  \u{23ce} apply \u{b7} esc cancel".to_string()
        } else {
            let extra = if it == Item::Template { " \u{b7} r reset" } else { "" };
            format!("  \u{2191}\u{2193} select \u{b7} \u{2190}\u{2192} change \u{b7} \u{23ce} edit/run{extra} \u{b7} esc close")
        };
        out.push(dim(truncate(&foot, width.saturating_sub(1))));
        (out, cursor)
    }

    pub(super) fn settings_key(&mut self, mut st: SettingsState, k: KeyEvent) {
        if let Some((kind, mut ed)) = st.edit.take() {
            match k.code {
                KeyCode::Esc => {}
                _ if ctrl_c(&k) => {}
                KeyCode::Enter => {
                    let t = ed.text().trim().to_string();
                    if !self.settings_apply_edit(&mut st, kind, &t) {
                        st.edit = Some((kind, ed));
                    }
                }
                _ => {
                    ed.handle_key(&k);
                    st.edit = Some((kind, ed));
                }
            }
            self.modal = Some(Modal::Settings(st));
            return;
        }
        let it = ITEMS[st.sel];
        match k.code {
            KeyCode::Esc => return self.close_settings(st),
            _ if ctrl_c(&k) => return self.close_settings(st),
            KeyCode::Up => st.sel = (st.sel + ITEMS.len() - 1) % ITEMS.len(),
            KeyCode::Down => st.sel = (st.sel + 1) % ITEMS.len(),
            KeyCode::Left => self.settings_change(&mut st, it, -1),
            KeyCode::Right => self.settings_change(&mut st, it, 1),
            KeyCode::Enter => self.settings_enter(&mut st, it),
            KeyCode::Char('r') if it == Item::Template => {
                self.cfg.template = None;
                self.settings_done(&mut st, "Template reset to default");
            }
            _ => {}
        }
        // Pickers opened from here keep the settings screen as their parent.
        if self.modal.is_none() {
            self.modal = Some(Modal::Settings(st));
        } else {
            self.parent = Some(Modal::Settings(st));
        }
    }

    fn settings_done(&mut self, st: &mut SettingsState, msg: &str) {
        self.save_cfg();
        st.changes += 1;
        st.status = Some((true, msg.to_string()));
    }

    fn settings_change(&mut self, st: &mut SettingsState, it: Item, dir: i32) {
        let s = &mut self.cfg.settings;
        let msg = match it {
            Item::RecentKeep => {
                s.recent_keep = cycle(&RECENT_KEEP, s.recent_keep, dir);
                let n = s.recent_keep;
                self.cfg.recent.truncate(n);
                format!("Keeping {n} recent folders")
            }
            Item::HistKeep => {
                s.history_keep = cycle(&HISTORY_KEEP, s.history_keep, dir);
                let n = s.history_keep;
                self.history.trim_to(n);
                self.persist_history();
                format!("Keeping {n} history entries")
            }
            Item::HistPersist => {
                s.history_persist = !s.history_persist;
                let on = s.history_persist;
                self.persist_history();
                self.sync_session_log();
                format!("History persistence {}", if on { "on" } else { "off" })
            }
            Item::AskSaveMethod => {
                s.ask_save_method = !s.ask_save_method;
                format!("Ask where to save {}", if s.ask_save_method { "on" } else { "off" })
            }
            Item::Force => {
                s.force = !s.force;
                format!("Overwrite existing {}", if s.force { "on" } else { "off" })
            }
            Item::Playlist => {
                s.playlist = !s.playlist;
                format!("Whole playlist {}", if s.playlist { "on" } else { "off" })
            }
            Item::ExitSummary => {
                s.exit_summary = !s.exit_summary;
                format!("Exit summary {}", if s.exit_summary { "on" } else { "off" })
            }
            Item::Cookies => {
                let mut opts = vec![None];
                opts.extend(BROWSERS.iter().map(|b| Some(b.to_string())));
                let i = opts.iter().position(|o| *o == s.cookies).unwrap_or(0) as i32;
                s.cookies = opts[(i + dir).rem_euclid(opts.len() as i32) as usize].clone();
                format!("Cookies from {}", s.cookies.as_deref().unwrap_or("no browser"))
            }
            _ => return,
        };
        self.settings_done(st, &msg);
    }

    fn settings_enter(&mut self, st: &mut SettingsState, it: Item) {
        match it {
            Item::DefFormat => self.modal = Some(Modal::Format(FormatPicker::new(&self.spec))),
            Item::DefDest => self.modal = Some(Modal::Dest(DestPicker::new(&self.cfg, &self.dest))),
            Item::ClearRecent => {
                self.cfg.recent.clear();
                self.settings_done(st, "Cleared recent folders");
            }
            Item::HistClear => {
                self.history.clear();
                let _ = save_history(&[]);
                st.changes += 1;
                st.status = Some((true, "Cleared input history".into()));
            }
            Item::HistTrim => {
                st.edit = Some((EditKind::TrimHistory, LineEditor::default()));
                st.status = Some((true, "Keep how many of the newest entries?".into()));
            }
            Item::Template => {
                let mut ed = LineEditor::default();
                ed.set(self.cfg.template.as_deref().unwrap_or(crate::format::DEFAULT_TEMPLATE));
                st.edit = Some((EditKind::Template, ed));
                st.status = None;
            }
            _ => self.settings_change(st, it, 1),
        }
    }

    fn settings_apply_edit(&mut self, st: &mut SettingsState, kind: EditKind, text: &str) -> bool {
        match kind {
            EditKind::Template => {
                if text.is_empty() || text == crate::format::DEFAULT_TEMPLATE {
                    self.cfg.template = None;
                } else if !text.contains("%(") {
                    st.status = Some((false, "a template needs at least one %(field)s".into()));
                    return false;
                } else {
                    self.cfg.template = Some(text.to_string());
                }
                self.settings_done(st, "Filename template updated");
                true
            }
            EditKind::TrimHistory => match text.parse::<usize>() {
                Ok(n) => {
                    let dropped = self.history.trim_to(n);
                    let _ = if self.cfg.settings.history_persist || history_path().exists() {
                        save_history(self.history.items())
                    } else {
                        Ok(())
                    };
                    st.changes += 1;
                    st.status = Some((true, format!("Kept the newest {n}, removed {dropped}")));
                    true
                }
                Err(_) => {
                    st.status = Some((false, "enter a number".into()));
                    false
                }
            },
        }
    }

    fn close_settings(&mut self, st: SettingsState) {
        if st.changes > 0 {
            self.push(heading("Settings updated"));
            self.push(detail(&format!("{} change{}", st.changes, if st.changes == 1 { "" } else { "s" })));
        }
        self.modal = None;
    }
}
