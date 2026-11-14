//! Codex-style selection lists (format and destination pickers) shared by the REPL and `--pick`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use unicode_width::UnicodeWidthStr;

use super::input::LineEditor;
use super::term::InlineTerm;
use super::{bold, cyan, dim, red, truncate};
use crate::config::{self, Config};
use crate::format::{Container, FormatSpec};

/// Visible rows of a list (the list scrolls beyond that).
const MAX_ROWS: usize = 8;
const MAX_CAND_ROWS: usize = 3;

pub enum Step<T> {
    Pending,
    Done(T),
    Cancel,
}

/// A modal widget living in the inline live area.
pub trait Interactive {
    type Out;
    fn key(&mut self, k: KeyEvent) -> Step<Self::Out>;
    fn paste(&mut self, s: &str);
    /// All lines of the widget (ANSI-styled). The count may change between calls.
    fn lines(&self, width: usize) -> Vec<String>;
    /// Cursor (column, row) relative to the first line.
    fn cursor(&self, width: usize) -> Option<(u16, u16)>;
    /// Show a validation error until the next key press.
    fn error(&mut self, _msg: String) {}
}

fn is_cancel(k: &KeyEvent) -> bool {
    k.code == KeyCode::Esc || (k.code == KeyCode::Char('c') && k.modifiers.contains(KeyModifiers::CONTROL))
}

// ---------------------------------------------------------------------------------------------
// Generic filterable list
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Item {
    pub label: String,
    pub detail: String,
    /// Extra text matched by the filter.
    pub search: String,
    /// Always listed, regardless of the filter.
    pub sticky: bool,
}

impl Item {
    pub fn new(label: impl Into<String>, detail: impl Into<String>) -> Self {
        Item {
            label: label.into(),
            detail: detail.into(),
            search: String::new(),
            sticky: false,
        }
    }
}

/// Every whitespace-separated token of `query` must occur in `hay` (case-insensitive).
pub fn matches_query(hay: &str, query: &str) -> bool {
    let hay = hay.to_lowercase();
    query.split_whitespace().all(|t| hay.contains(&t.to_lowercase()))
}

pub struct Picker {
    title: String,
    items: Vec<Item>,
    filter: LineEditor,
    /// Item indices passing the filter.
    matches: Vec<usize>,
    /// Index into `matches`.
    sel: usize,
    top: usize,
    rows: usize,
    error: Option<String>,
}

impl Picker {
    /// `current` is the item index to preselect.
    pub fn new(title: impl Into<String>, items: Vec<Item>, current: Option<usize>) -> Self {
        let rows = items.len().clamp(1, MAX_ROWS);
        let mut p = Picker {
            title: title.into(),
            matches: (0..items.len()).collect(),
            items,
            filter: LineEditor::default(),
            sel: current.unwrap_or(0),
            top: 0,
            rows,
            error: None,
        };
        p.sel = p.sel.min(p.matches.len().saturating_sub(1));
        // Show the preselected row in the middle of the window when the list scrolls.
        p.top = p.sel.saturating_sub(p.rows / 2).min(p.matches.len().saturating_sub(p.rows));
        p.scroll_into_view();
        p
    }

    pub fn matches(&self) -> &[usize] {
        &self.matches
    }

    pub fn filter_text(&self) -> &str {
        self.filter.text()
    }

    /// Item index of the highlighted row.
    pub fn selected(&self) -> Option<usize> {
        self.matches.get(self.sel).copied()
    }

    fn refilter(&mut self) {
        let q = self.filter.text().to_string();
        self.matches = (0..self.items.len())
            .filter(|&i| {
                let it = &self.items[i];
                it.sticky || matches_query(&format!("{} {} {}", it.label, it.detail, it.search), &q)
            })
            .collect();
        // Prefer a real match over a sticky row.
        self.sel = self
            .matches
            .iter()
            .position(|&i| !self.items[i].sticky)
            .unwrap_or(0)
            .min(self.matches.len().saturating_sub(1));
        self.top = 0;
        self.scroll_into_view();
    }

    fn scroll_into_view(&mut self) {
        if self.sel < self.top {
            self.top = self.sel;
        } else if self.sel >= self.top + self.rows {
            self.top = self.sel + 1 - self.rows;
        }
    }

    fn move_sel(&mut self, delta: isize) {
        if self.matches.is_empty() {
            return;
        }
        let n = self.matches.len() as isize;
        self.sel = (self.sel as isize + delta).rem_euclid(n) as usize;
        self.scroll_into_view();
    }

    fn move_sel_clamped(&mut self, delta: isize) {
        if self.matches.is_empty() {
            return;
        }
        let n = self.matches.len() as isize;
        self.sel = (self.sel as isize + delta).clamp(0, n - 1) as usize;
        self.scroll_into_view();
    }

    pub fn set_error(&mut self, msg: String) {
        self.error = Some(msg);
    }

    pub fn key(&mut self, k: KeyEvent) -> Step<usize> {
        self.error = None;
        if is_cancel(&k) {
            return Step::Cancel;
        }
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        match k.code {
            KeyCode::Enter => {
                return match self.selected() {
                    Some(i) => Step::Done(i),
                    None => Step::Pending,
                };
            }
            KeyCode::Up => self.move_sel(-1),
            KeyCode::Down => self.move_sel(1),
            KeyCode::Char('p') if ctrl => self.move_sel(-1),
            KeyCode::Char('n') if ctrl => self.move_sel(1),
            KeyCode::PageUp => self.move_sel_clamped(-(self.rows as isize)),
            KeyCode::PageDown => self.move_sel_clamped(self.rows as isize),
            KeyCode::Tab => self.move_sel(1),
            KeyCode::BackTab => self.move_sel(-1),
            _ => {
                let before = self.filter.text().to_string();
                self.filter.handle_key(&k);
                if self.filter.text() != before {
                    self.refilter();
                }
            }
        }
        Step::Pending
    }

    pub fn paste(&mut self, s: &str) {
        self.filter.insert_str(s.trim());
        self.refilter();
    }

    fn title_line(&self, width: usize) -> (String, u16) {
        let tw = self.title.width();
        let mut line = format!("  {}", bold(&self.title));
        let (shown, col) = self.filter.window(width.saturating_sub(tw + 5));
        if !self.filter.is_empty() {
            line.push_str(&format!("  {}", cyan(&shown)));
        }
        (line, (tw + 4 + col) as u16)
    }

    pub fn lines(&self, width: usize) -> Vec<String> {
        let width = width.max(20);
        let mut out = vec![self.title_line(width).0];
        let label_w = self.items.iter().map(|i| i.label.width()).max().unwrap_or(0).min((width * 2 / 3).max(32));
        if self.matches.is_empty() {
            out.push(dim("    no matches"));
        }
        for row in 0..self.rows {
            let Some(&i) = self.matches.get(self.top + row) else {
                if !self.matches.is_empty() {
                    out.push(String::new());
                }
                continue;
            };
            let it = &self.items[i];
            // Rows without a detail may use the whole width (long paths).
            let limit = if it.detail.is_empty() { width.saturating_sub(5).max(label_w) } else { label_w };
            let label = truncate(&it.label, limit);
            let pad = " ".repeat(label_w.saturating_sub(label.width()));
            let detail_w = width.saturating_sub(4 + label_w + 2);
            let detail = truncate(&it.detail, detail_w);
            if self.top + row == self.sel {
                out.push(format!("  {} {}{}  {}", cyan("\u{203a}"), cyan(&label), pad, dim(&detail)));
            } else {
                out.push(format!("    {label}{pad}  {}", dim(&detail)));
            }
        }
        if self.matches.is_empty() {
            for _ in 1..self.rows {
                out.push(String::new());
            }
        }
        out.push(match &self.error {
            Some(e) => format!("  {}", red(truncate(e, width.saturating_sub(4)))),
            None => dim(truncate(
                "  type to filter \u{b7} \u{2191}\u{2193} select \u{b7} \u{23ce} confirm \u{b7} esc cancel",
                width.saturating_sub(1),
            )),
        });
        out
    }

    pub fn cursor(&self, width: usize) -> Option<(u16, u16)> {
        Some((self.title_line(width.max(20)).1, 0))
    }
}

// ---------------------------------------------------------------------------------------------
// Format picker
// ---------------------------------------------------------------------------------------------

pub struct FormatPicker {
    picker: Picker,
    choices: Vec<FormatSpec>,
}

fn format_detail(spec: &FormatSpec) -> String {
    let c = spec.container;
    if c == Container::Original {
        "no conversion".into()
    } else if c.is_audio() {
        if c.is_lossless() { "audio \u{b7} lossless".into() } else { "audio".into() }
    } else if c == Container::Gif {
        "animation".into()
    } else {
        "video".into()
    }
}

impl FormatPicker {
    pub fn new(current: &FormatSpec) -> Self {
        let choices = FormatSpec::all_choices();
        let items = choices
            .iter()
            .map(|s| {
                let mut it = Item::new(s.to_string(), format_detail(s));
                it.search = s.canonical();
                it
            })
            .collect();
        let cur = choices.iter().position(|s| s == current);
        FormatPicker {
            picker: Picker::new("Select format", items, cur),
            choices,
        }
    }
}

impl Interactive for FormatPicker {
    type Out = FormatSpec;

    fn key(&mut self, k: KeyEvent) -> Step<FormatSpec> {
        match self.picker.key(k) {
            Step::Done(i) => Step::Done(self.choices[i]),
            Step::Cancel => Step::Cancel,
            Step::Pending => Step::Pending,
        }
    }

    fn paste(&mut self, s: &str) {
        self.picker.paste(s);
    }

    fn lines(&self, width: usize) -> Vec<String> {
        self.picker.lines(width)
    }

    fn cursor(&self, width: usize) -> Option<(u16, u16)> {
        self.picker.cursor(width)
    }

    fn error(&mut self, msg: String) {
        self.picker.set_error(msg);
    }
}

// ---------------------------------------------------------------------------------------------
// Path input with Tab completion
// ---------------------------------------------------------------------------------------------

#[derive(Debug, PartialEq, Eq)]
pub struct Completion {
    pub text: String,
    /// Directory names (with trailing `/`) when the completion is ambiguous.
    pub candidates: Vec<String>,
}

fn common_prefix(names: &[String]) -> String {
    let mut p = names[0].clone();
    for n in &names[1..] {
        let len = p.chars().zip(n.chars()).take_while(|(a, b)| a == b).count();
        p = p.chars().take(len).collect();
    }
    p
}

/// Complete the last path component of `input` against directories (`~` is kept as typed).
pub fn complete_path(input: &str) -> Completion {
    if input.is_empty() {
        return Completion {
            text: "~/".into(),
            candidates: vec![],
        };
    }
    if input == "~" {
        return Completion {
            text: "~/".into(),
            candidates: vec![],
        };
    }
    let (dir_str, prefix) = match input.rfind('/') {
        Some(i) => (&input[..=i], &input[i + 1..]),
        None => ("", input),
    };
    let real = if dir_str.is_empty() { PathBuf::from(".") } else { config::expand_tilde(dir_str) };
    let mut names: Vec<String> = std::fs::read_dir(&real)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| std::fs::metadata(e.path()).is_ok_and(|m| m.is_dir()))
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.starts_with(prefix) && (prefix.starts_with('.') || !n.starts_with('.')))
        .collect();
    names.sort();
    match names.len() {
        0 => Completion {
            text: input.to_string(),
            candidates: vec![],
        },
        1 => Completion {
            text: format!("{dir_str}{}/", names[0]),
            candidates: vec![],
        },
        _ => {
            let lcp = common_prefix(&names);
            Completion {
                text: format!("{dir_str}{lcp}"),
                candidates: names.into_iter().map(|n| format!("{n}/")).collect(),
            }
        }
    }
}

pub struct PathInput {
    ed: LineEditor,
    cands: Vec<String>,
    error: Option<String>,
}

impl PathInput {
    pub fn new() -> Self {
        PathInput {
            ed: LineEditor::default(),
            cands: Vec::new(),
            error: None,
        }
    }

    pub fn text(&self) -> &str {
        self.ed.text()
    }

    /// `Done` carries the raw text, `Cancel` means "back".
    pub fn key(&mut self, k: KeyEvent) -> Step<String> {
        self.error = None;
        if is_cancel(&k) {
            return Step::Cancel;
        }
        match k.code {
            KeyCode::Enter => {
                let t = self.ed.text().trim().to_string();
                if !t.is_empty() {
                    return Step::Done(t);
                }
            }
            KeyCode::Tab => {
                let c = complete_path(self.ed.text());
                self.ed.set(&c.text);
                self.cands = c.candidates;
            }
            _ => {
                if self.ed.handle_key(&k) {
                    self.cands.clear();
                }
            }
        }
        Step::Pending
    }

    pub fn paste(&mut self, s: &str) {
        self.ed.insert_str(s.trim());
        self.cands.clear();
    }

    fn candidate_lines(&self, width: usize) -> Vec<String> {
        let mut rows: Vec<String> = Vec::new();
        let mut cur = String::new();
        let mut shown = 0;
        for c in &self.cands {
            let c = truncate(c, width.saturating_sub(6));
            if !cur.is_empty() && cur.width() + 2 + c.width() > width.saturating_sub(4) {
                rows.push(std::mem::take(&mut cur));
            }
            if rows.len() >= MAX_CAND_ROWS {
                break;
            }
            if !cur.is_empty() {
                cur.push_str("  ");
            }
            cur.push_str(&c);
            shown += 1;
        }
        if rows.len() < MAX_CAND_ROWS && !cur.is_empty() {
            rows.push(cur);
        }
        if shown < self.cands.len()
            && let Some(last) = rows.last_mut()
        {
            last.push_str(&format!("  \u{2026}+{}", self.cands.len() - shown));
        }
        rows.into_iter().map(|r| dim(format!("    {r}"))).collect()
    }

    pub fn lines(&self, width: usize) -> Vec<String> {
        let width = width.max(20);
        let mut out = vec![format!("  {}", bold("Enter a folder"))];
        let (shown, _) = self.ed.window(width.saturating_sub(5));
        if self.ed.is_empty() {
            out.push(format!("  {} {}", cyan("\u{203a}"), dim("~/Music, ./videos, /mnt/data \u{2026}")));
        } else {
            out.push(format!("  {} {shown}", cyan("\u{203a}")));
        }
        out.extend(self.candidate_lines(width));
        out.push(match &self.error {
            Some(e) => format!("  {}", red(truncate(e, width.saturating_sub(4)))),
            None => dim("  tab complete \u{b7} \u{23ce} confirm \u{b7} esc back"),
        });
        out
    }

    pub fn cursor(&self, width: usize) -> Option<(u16, u16)> {
        let (_, col) = self.ed.window(width.max(20).saturating_sub(5));
        Some(((4 + col) as u16, 1))
    }
}

impl Default for PathInput {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------------------------
// Destination picker
// ---------------------------------------------------------------------------------------------

enum DestStage {
    List,
    Input,
}

pub struct DestPicker {
    stage: DestStage,
    picker: Picker,
    /// Per list item: the path to use; `None` for the "Enter a path" row.
    paths: Vec<Option<String>>,
    input: PathInput,
}

impl DestPicker {
    pub fn new(cfg: &Config, current: &Path) -> Self {
        let mut items = Vec::new();
        let mut paths = Vec::new();
        let mut cur = None;
        let is_cur = |p: &Path| p == current;
        let fav_paths: Vec<&PathBuf> = cfg.favorites.iter().map(|f| &f.path).collect();
        // The current destination always appears, even when it is neither favorite nor recent.
        let known = fav_paths.iter().any(|p| is_cur(p)) || cfg.recent.iter().any(|p| is_cur(p));
        if !known {
            let mut it = Item::new(format!("\u{b7} {}", super::abbrev_home(current)), "current");
            it.search = current.display().to_string();
            items.push(it);
            paths.push(Some(current.display().to_string()));
            cur = Some(0);
        }
        for (i, f) in cfg.favorites.iter().enumerate() {
            if is_cur(&f.path) {
                cur = Some(items.len());
            }
            let mut it = Item::new(format!("f:{i} \u{2605} {}", f.name), super::abbrev_home(&f.path));
            it.search = f.path.display().to_string();
            items.push(it);
            paths.push(Some(f.path.display().to_string()));
        }
        for (i, r) in cfg.recent.iter().enumerate().filter(|(_, r)| !fav_paths.contains(r)) {
            if is_cur(r) {
                cur = Some(items.len());
            }
            let mut it = Item::new(format!("r:{i} \u{b7} {}", super::abbrev_home(r)), "");
            it.search = r.display().to_string();
            items.push(it);
            paths.push(Some(r.display().to_string()));
        }
        let mut enter = Item::new("+ Enter a path\u{2026}", "");
        enter.sticky = true;
        items.push(enter);
        paths.push(None);
        DestPicker {
            stage: DestStage::List,
            picker: Picker::new("Save to", items, cur),
            paths,
            input: PathInput::new(),
        }
    }
}

impl Interactive for DestPicker {
    type Out = String;

    fn key(&mut self, k: KeyEvent) -> Step<String> {
        match self.stage {
            DestStage::List => match self.picker.key(k) {
                Step::Done(i) => match &self.paths[i] {
                    Some(p) => Step::Done(p.clone()),
                    None => {
                        self.stage = DestStage::Input;
                        Step::Pending
                    }
                },
                Step::Cancel => Step::Cancel,
                Step::Pending => Step::Pending,
            },
            DestStage::Input => match self.input.key(k) {
                Step::Cancel => {
                    self.stage = DestStage::List;
                    Step::Pending
                }
                other => other,
            },
        }
    }

    fn paste(&mut self, s: &str) {
        match self.stage {
            DestStage::List => self.picker.paste(s),
            DestStage::Input => self.input.paste(s),
        }
    }

    fn lines(&self, width: usize) -> Vec<String> {
        match self.stage {
            DestStage::List => self.picker.lines(width),
            DestStage::Input => self.input.lines(width),
        }
    }

    fn cursor(&self, width: usize) -> Option<(u16, u16)> {
        match self.stage {
            DestStage::List => self.picker.cursor(width),
            DestStage::Input => self.input.cursor(width),
        }
    }

    fn error(&mut self, msg: String) {
        match self.stage {
            DestStage::List => self.picker.set_error(msg),
            DestStage::Input => self.input.error = Some(msg),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Blocking runner (one-shot `--pick`)
// ---------------------------------------------------------------------------------------------

/// Run one picker in its own inline viewport until it is confirmed (`Some`) or cancelled (`None`).
/// `validate` may reject a confirmed value (the message is shown inside the widget).
pub fn run<I, T>(mut w: I, mut validate: impl FnMut(I::Out) -> std::result::Result<T, String>) -> Result<Option<T>>
where
    I: Interactive,
    I::Out: Clone,
{
    let mut term = InlineTerm::new(3)?;
    let result = (|| -> Result<Option<T>> {
        loop {
            let width = term.width();
            term.render(&w.lines(width), w.cursor(width))?;
            if !event::poll(Duration::from_millis(250))? {
                continue;
            }
            match event::read()? {
                Event::Key(k) if k.kind != KeyEventKind::Release => match w.key(k) {
                    Step::Pending => {}
                    Step::Cancel => return Ok(None),
                    Step::Done(out) => match validate(out) {
                        Ok(v) => return Ok(Some(v)),
                        Err(msg) => w.error(msg),
                    },
                },
                Event::Paste(s) => w.paste(&s),
                _ => {}
            }
        }
    })();
    term.finish();
    result
}

/// Convenience used by `--pick`: ask for a format. `None` = cancelled.
pub fn pick_format(current: &FormatSpec) -> Result<Option<FormatSpec>> {
    run(FormatPicker::new(current), Ok)
}

/// Convenience used by `--pick`: ask for a destination and create it. `None` = cancelled.
pub fn pick_dest(cfg: &Config, current: &Path) -> Result<Option<PathBuf>> {
    run(DestPicker::new(cfg, current), |s| cfg.resolve_dest(&s).map_err(|e| format!("{e:#}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::ansi;
    use crossterm::event::KeyEventKind;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new_with_kind(code, KeyModifiers::NONE, KeyEventKind::Press)
    }

    fn type_str(p: &mut Picker, s: &str) {
        for c in s.chars() {
            p.key(key(KeyCode::Char(c)));
        }
    }

    fn labels(p: &Picker) -> Vec<String> {
        p.matches().iter().map(|&i| p.items[i].label.clone()).collect()
    }

    #[test]
    fn query_matching() {
        assert!(matches_query("MP3 \u{b7} 320k audio", "mp3 320"));
        assert!(!matches_query("mp3 \u{b7} best", "flac"));
        assert!(matches_query("anything", ""));
    }

    #[test]
    fn format_picker_preselects_and_filters() {
        let cur = FormatSpec::parse("mp3:320").unwrap();
        let mut fp = FormatPicker::new(&cur);
        assert_eq!(fp.picker.items[fp.picker.selected().unwrap()].label, "mp3 \u{b7} 320k");
        type_str(&mut fp.picker, "1080");
        assert!(labels(&fp.picker).iter().all(|l| l.contains("1080p")));
        assert_eq!(fp.picker.matches().len(), 5);
        match fp.key(key(KeyCode::Enter)) {
            Step::Done(s) => assert_eq!(s.canonical(), "mp4:1080"),
            _ => panic!("expected confirm"),
        }
        let mut fp = FormatPicker::new(&cur);
        type_str(&mut fp.picker, "zzzz");
        assert!(fp.picker.matches().is_empty());
        assert!(matches!(fp.key(key(KeyCode::Enter)), Step::Pending));
        assert!(matches!(fp.key(key(KeyCode::Esc)), Step::Cancel));
        // Editing the filter back restores the list.
        for _ in 0..4 {
            fp.picker.key(key(KeyCode::Backspace));
        }
        assert_eq!(fp.picker.matches().len(), FormatSpec::all_choices().len());
    }

    #[test]
    fn selection_wraps_and_scrolls() {
        let items: Vec<Item> = (0..20).map(|i| Item::new(format!("item{i}"), "")).collect();
        let mut p = Picker::new("t", items, Some(0));
        p.key(key(KeyCode::Up));
        assert_eq!(p.selected(), Some(19));
        assert!(p.top + p.rows > 19);
        assert_eq!(p.lines(60).len(), p.rows + 2);
        p.key(key(KeyCode::Down));
        assert_eq!(p.selected(), Some(0));
        assert_eq!(p.top, 0);
    }

    #[test]
    fn dest_picker_lists_favorites_recent_and_enter() {
        let mut cfg = Config::default();
        cfg.favorites.push(config::Favorite {
            name: "music".into(),
            path: PathBuf::from("/m"),
        });
        cfg.recent = vec![PathBuf::from("/m"), PathBuf::from("/r1"), PathBuf::from("/r2")];
        let mut dp = DestPicker::new(&cfg, Path::new("/r1"));
        let l: Vec<String> = dp.picker.items.iter().map(|i| i.label.clone()).collect();
        assert_eq!(l, ["f:0 \u{2605} music", "r:1 \u{b7} /r1", "r:2 \u{b7} /r2", "+ Enter a path\u{2026}"]);
        assert_eq!(dp.picker.selected(), Some(1));
        // Filtering keeps the sticky row; Enter on it switches to the input stage.
        for c in "r2".chars() {
            dp.key(key(KeyCode::Char(c)));
        }
        assert_eq!(dp.picker.matches().len(), 2);
        assert!(matches!(dp.key(key(KeyCode::Enter)), Step::Done(p) if p == "/r2"));
        let mut dp = DestPicker::new(&cfg, Path::new("/elsewhere"));
        assert_eq!(dp.picker.items[0].detail, "current");
        for c in "zzz".chars() {
            dp.key(key(KeyCode::Char(c)));
        }
        assert!(matches!(dp.key(key(KeyCode::Enter)), Step::Pending));
        assert!(matches!(dp.stage, DestStage::Input));
        for c in "/tmp".chars() {
            dp.key(key(KeyCode::Char(c)));
        }
        assert!(matches!(dp.key(key(KeyCode::Enter)), Step::Done(p) if p == "/tmp"));
    }

    #[test]
    fn path_completion() {
        let base = std::env::temp_dir().join(format!("yd-complete-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        for d in ["alpha", "alpine", "beta", ".hidden"] {
            std::fs::create_dir_all(base.join(d)).unwrap();
        }
        std::fs::write(base.join("alfile"), "").unwrap();
        let b = base.display().to_string();
        let c = complete_path(&format!("{b}/be"));
        assert_eq!(c.text, format!("{b}/beta/"));
        assert!(c.candidates.is_empty());
        let c = complete_path(&format!("{b}/al"));
        assert_eq!(c.text, format!("{b}/alp"));
        assert_eq!(c.candidates, ["alpha/", "alpine/"]);
        let c = complete_path(&format!("{b}/alph"));
        assert_eq!(c.text, format!("{b}/alpha/"));
        assert_eq!(complete_path(&format!("{b}/zz")).text, format!("{b}/zz"));
        let c = complete_path(&format!("{b}/"));
        assert_eq!(c.candidates, ["alpha/", "alpine/", "beta/"]);
        assert_eq!(complete_path(&format!("{b}/.h")).text, format!("{b}/.hidden/"));
        assert_eq!(complete_path("~").text, "~/");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn path_input_flow() {
        let mut pi = PathInput::new();
        assert!(matches!(pi.key(key(KeyCode::Enter)), Step::Pending));
        pi.paste("  ~/x ");
        assert_eq!(pi.text(), "~/x");
        assert!(matches!(pi.key(key(KeyCode::Enter)), Step::Done(s) if s == "~/x"));
        assert!(matches!(pi.key(key(KeyCode::Esc)), Step::Cancel));
        assert!(pi.lines(60).len() >= 3);
    }

    #[test]
    fn line_widths_fit() {
        let fp = FormatPicker::new(&FormatSpec::DEFAULT);
        for w in [20, 40, 80] {
            assert!(fp.lines(w).iter().all(|l| ansi::width(l) <= w));
        }
    }
}
