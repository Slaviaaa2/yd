//! Single-line text editing (composer, picker filter, path input) and input history.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

#[derive(Debug, Clone, Default)]
pub struct LineEditor {
    text: String,
    /// Byte offset of the cursor (always on a char boundary).
    cur: usize,
}

impl LineEditor {
    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    pub fn cursor(&self) -> usize {
        self.cur
    }

    pub fn set(&mut self, s: &str) {
        self.text = s.to_string();
        self.cur = self.text.len();
    }

    pub fn clear(&mut self) {
        self.text.clear();
        self.cur = 0;
    }

    /// Insert text at the cursor. Newlines and tabs become spaces, other control chars are dropped.
    pub fn insert_str(&mut self, s: &str) {
        for c in s.chars() {
            self.insert_char(c);
        }
    }

    pub fn insert_char(&mut self, c: char) {
        let c = match c {
            '\n' | '\r' | '\t' => ' ',
            c if c.is_control() => return,
            c => c,
        };
        self.text.insert(self.cur, c);
        self.cur += c.len_utf8();
    }

    fn prev_boundary(&self, from: usize) -> usize {
        self.text[..from].char_indices().next_back().map(|(i, _)| i).unwrap_or(0)
    }

    fn next_boundary(&self, from: usize) -> usize {
        self.text[from..].chars().next().map(|c| from + c.len_utf8()).unwrap_or(from)
    }

    pub fn backspace(&mut self) {
        if self.cur > 0 {
            let p = self.prev_boundary(self.cur);
            self.text.replace_range(p..self.cur, "");
            self.cur = p;
        }
    }

    pub fn delete(&mut self) {
        if self.cur < self.text.len() {
            let n = self.next_boundary(self.cur);
            self.text.replace_range(self.cur..n, "");
        }
    }

    pub fn left(&mut self) {
        self.cur = self.prev_boundary(self.cur);
    }

    pub fn right(&mut self) {
        self.cur = self.next_boundary(self.cur);
    }

    pub fn home(&mut self) {
        self.cur = 0;
    }

    pub fn end(&mut self) {
        self.cur = self.text.len();
    }

    fn word_start(&self) -> usize {
        let mut i = self.cur;
        while i > 0 {
            let p = self.prev_boundary(i);
            if self.text[p..i].chars().all(char::is_whitespace) {
                i = p;
            } else {
                break;
            }
        }
        while i > 0 {
            let p = self.prev_boundary(i);
            if self.text[p..i].chars().all(char::is_whitespace) {
                break;
            }
            i = p;
        }
        i
    }

    pub fn word_left(&mut self) {
        self.cur = self.word_start();
    }

    pub fn word_right(&mut self) {
        let mut i = self.cur;
        while i < self.text.len() {
            let n = self.next_boundary(i);
            if self.text[i..n].chars().all(char::is_whitespace) {
                i = n;
            } else {
                break;
            }
        }
        while i < self.text.len() {
            let n = self.next_boundary(i);
            if self.text[i..n].chars().all(char::is_whitespace) {
                break;
            }
            i = n;
        }
        self.cur = i;
    }

    pub fn delete_word_back(&mut self) {
        let s = self.word_start();
        self.text.replace_range(s..self.cur, "");
        self.cur = s;
    }

    pub fn kill_to_end(&mut self) {
        self.text.truncate(self.cur);
    }

    /// Apply an editing key. Returns whether the key was an editing key.
    pub fn handle_key(&mut self, k: &KeyEvent) -> bool {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let alt = k.modifiers.contains(KeyModifiers::ALT);
        match k.code {
            KeyCode::Char(c) if ctrl => match c.to_ascii_lowercase() {
                'a' => self.home(),
                'e' => self.end(),
                'u' => self.clear(),
                'w' => self.delete_word_back(),
                'k' => self.kill_to_end(),
                'b' => self.left(),
                'f' => self.right(),
                'd' => self.delete(),
                'h' => self.backspace(),
                _ => return false,
            },
            KeyCode::Char(c) if alt => match c {
                'b' => self.word_left(),
                'f' => self.word_right(),
                _ => return false,
            },
            KeyCode::Char(c) => self.insert_char(c),
            KeyCode::Backspace if ctrl || alt => self.delete_word_back(),
            KeyCode::Backspace => self.backspace(),
            KeyCode::Delete => self.delete(),
            KeyCode::Left if ctrl || alt => self.word_left(),
            KeyCode::Right if ctrl || alt => self.word_right(),
            KeyCode::Left => self.left(),
            KeyCode::Right => self.right(),
            KeyCode::Home => self.home(),
            KeyCode::End => self.end(),
            _ => return false,
        }
        true
    }

    /// Display column of the cursor.
    pub fn cursor_col(&self) -> usize {
        self.text[..self.cur].width()
    }

    /// Byte range of the slice to show in `avail` columns (cursor cell included) and the cursor
    /// column within it.
    pub fn window_range(&self, avail: usize) -> (usize, usize, usize) {
        let avail = avail.max(2);
        if self.text.width() < avail {
            return (0, self.text.len(), self.cursor_col());
        }
        // Keep the cursor visible; leave some text to its right when possible.
        let back_budget = avail * 3 / 4;
        let mut start = self.cur;
        let mut w = 0;
        for (i, c) in self.text[..self.cur].char_indices().rev() {
            let cw = c.width().unwrap_or(0);
            if w + cw > back_budget {
                break;
            }
            w += cw;
            start = i;
        }
        let mut end = start;
        let mut used = 0;
        for c in self.text[start..].chars() {
            let cw = c.width().unwrap_or(0);
            if used + cw > avail - 1 {
                break;
            }
            end += c.len_utf8();
            used += cw;
        }
        (start, end, self.text[start..self.cur].width())
    }

    /// The slice to show in `avail` columns and the cursor column within it.
    pub fn window(&self, avail: usize) -> (String, usize) {
        let (s, e, col) = self.window_range(avail);
        (self.text[s..e].to_string(), col)
    }
}

/// In-session input history (up/down).
#[derive(Debug, Default)]
pub struct History {
    items: Vec<String>,
    idx: Option<usize>,
    draft: String,
}

impl History {
    pub fn push(&mut self, s: &str) {
        let s = s.trim();
        if !s.is_empty() && self.items.last().map(String::as_str) != Some(s) {
            self.items.push(s.to_string());
        }
        self.idx = None;
    }

    pub fn prev(&mut self, current: &str) -> Option<String> {
        match self.idx {
            None => {
                if self.items.is_empty() {
                    return None;
                }
                self.draft = current.to_string();
                self.idx = Some(self.items.len() - 1);
            }
            Some(0) => return None,
            Some(i) => self.idx = Some(i - 1),
        }
        self.idx.map(|i| self.items[i].clone())
    }

    pub fn next(&mut self) -> Option<String> {
        let i = self.idx?;
        if i + 1 < self.items.len() {
            self.idx = Some(i + 1);
            Some(self.items[i + 1].clone())
        } else {
            self.idx = None;
            Some(std::mem::take(&mut self.draft))
        }
    }

    pub fn reset(&mut self) {
        self.idx = None;
    }

    pub fn from_items(items: Vec<String>) -> Self {
        History {
            items,
            ..History::default()
        }
    }

    pub fn items(&self) -> &[String] {
        &self.items
    }

    /// Keep only the newest `n` entries; returns how many were dropped.
    pub fn trim_to(&mut self, n: usize) -> usize {
        let drop = self.items.len().saturating_sub(n);
        self.items.drain(..drop);
        self.idx = None;
        drop
    }

    pub fn clear(&mut self) {
        self.items.clear();
        self.idx = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEventKind;

    fn key(code: KeyCode, m: KeyModifiers) -> KeyEvent {
        KeyEvent::new_with_kind(code, m, KeyEventKind::Press)
    }

    fn ed(s: &str) -> LineEditor {
        let mut e = LineEditor::default();
        e.set(s);
        e
    }

    #[test]
    fn basic_editing_with_wide_chars() {
        let mut e = ed("");
        e.insert_str("a日本b");
        assert_eq!(e.cursor_col(), 6);
        e.left();
        e.left();
        assert_eq!(e.cursor_col(), 3);
        e.backspace();
        assert_eq!(e.text(), "a本b");
        e.delete();
        assert_eq!(e.text(), "ab");
        e.home();
        assert_eq!(e.cursor(), 0);
        e.end();
        assert_eq!(e.cursor(), 2);
    }

    #[test]
    fn word_ops() {
        let mut e = ed("foo bar  baz");
        e.delete_word_back();
        assert_eq!(e.text(), "foo bar  ");
        e.delete_word_back();
        assert_eq!(e.text(), "foo ");
        e.home();
        e.word_right();
        assert_eq!(e.cursor(), 3);
        e.word_right();
        assert_eq!(e.cursor(), 4);
    }

    #[test]
    fn keys_and_paste_sanitising() {
        let mut e = ed("hello world");
        assert!(e.handle_key(&key(KeyCode::Char('w'), KeyModifiers::CONTROL)));
        assert_eq!(e.text(), "hello ");
        assert!(e.handle_key(&key(KeyCode::Char('u'), KeyModifiers::CONTROL)));
        assert!(e.is_empty());
        e.insert_str("a\nb\tc\x07");
        assert_eq!(e.text(), "a b c");
        assert!(!e.handle_key(&key(KeyCode::Enter, KeyModifiers::NONE)));
    }

    #[test]
    fn window_keeps_cursor_visible() {
        let e = ed(&"x".repeat(100));
        let (s, col) = e.window(20);
        assert!(s.width() < 20 && col < 20);
        let mut e = ed(&"日".repeat(40));
        e.home();
        let (s, col) = e.window(11);
        assert_eq!(col, 0);
        assert!(s.width() <= 10);
        e.end();
        let (_, col) = e.window(11);
        assert!(col <= 10);
        let (s, col) = ed("short").window(20);
        assert_eq!((s.as_str(), col), ("short", 5));
    }

    #[test]
    fn history_trim_and_clear() {
        let mut h = History::from_items((0..10).map(|i| i.to_string()).collect());
        assert_eq!(h.trim_to(3), 7);
        assert_eq!(h.items(), ["7", "8", "9"]);
        assert_eq!(h.trim_to(50), 0);
        h.push("x");
        assert_eq!(h.items().len(), 4);
        h.clear();
        assert!(h.items().is_empty());
    }

    #[test]
    fn history_navigation() {
        let mut h = History::default();
        assert_eq!(h.prev("x"), None);
        h.push("one");
        h.push("two");
        h.push("two");
        assert_eq!(h.prev("draft").as_deref(), Some("two"));
        assert_eq!(h.prev("").as_deref(), Some("one"));
        assert_eq!(h.prev(""), None);
        assert_eq!(h.next().as_deref(), Some("two"));
        assert_eq!(h.next().as_deref(), Some("draft"));
        assert_eq!(h.next(), None);
    }
}
