//! Inline terminal (ratatui `Viewport::Inline`): finished output goes to normal scrollback via
//! `insert_before`; only the live area at the bottom is redrawn. The viewport height follows
//! the content, so a height change re-anchors the viewport at its current top row.

use std::io::{self, IsTerminal, Stdout, Write};

use anyhow::{Result, bail};
use crossterm::cursor::{MoveTo, Show};
use crossterm::event::{DisableBracketedPaste, EnableBracketedPaste};
use crossterm::terminal::{Clear, ClearType, disable_raw_mode, enable_raw_mode};
use crossterm::{execute, queue};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Position;
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Widget};
use ratatui::{Terminal, TerminalOptions, Viewport};

use super::ansi;

/// Lines per `insert_before` call (keeps buffers small for huge outputs).
const PRINT_CHUNK: usize = 200;

/// Undo raw mode / bracketed paste / hidden cursor. Safe to call repeatedly and from a panic hook.
pub fn restore_terminal() {
    let _ = execute!(io::stdout(), DisableBracketedPaste, Show);
    let _ = disable_raw_mode();
}

fn install_panic_hook() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        prev(info);
    }));
}

pub struct InlineTerm {
    term: Terminal<CrosstermBackend<Stdout>>,
    height: u16,
}

fn make_terminal(height: u16) -> io::Result<Terminal<CrosstermBackend<Stdout>>> {
    Terminal::with_options(
        CrosstermBackend::new(io::stdout()),
        TerminalOptions {
            viewport: Viewport::Inline(height),
        },
    )
}

impl InlineTerm {
    pub fn new(height: u16) -> Result<Self> {
        if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
            bail!("this needs an interactive terminal");
        }
        install_panic_hook();
        enable_raw_mode()?;
        let setup = || -> io::Result<Self> {
            execute!(io::stdout(), EnableBracketedPaste)?;
            let (_, th) = crossterm::terminal::size()?;
            let height = height.clamp(1, th.max(1));
            Ok(InlineTerm {
                term: make_terminal(height)?,
                height,
            })
        };
        match setup() {
            Ok(t) => Ok(t),
            Err(e) => {
                restore_terminal();
                Err(e.into())
            }
        }
    }

    pub fn width(&self) -> usize {
        crossterm::terminal::size().map(|(w, _)| w as usize).unwrap_or(80).max(1)
    }

    /// Move the viewport to `h` rows (clamped to the screen), anchored at its current top.
    fn set_height(&mut self, h: u16) -> io::Result<()> {
        let (_, th) = crossterm::terminal::size()?;
        let h = h.clamp(1, th.max(1));
        if h == self.height {
            return Ok(());
        }
        self.term.autoresize()?;
        let top = self.term.get_frame().area().y;
        let mut out = io::stdout();
        queue!(out, MoveTo(0, top), Clear(ClearType::FromCursorDown))?;
        out.flush()?;
        self.term = make_terminal(h)?;
        self.height = h;
        Ok(())
    }

    /// Redraw the live area. `cursor` is (column, row) relative to the live area.
    pub fn render(&mut self, lines: &[String], cursor: Option<(u16, u16)>) -> io::Result<()> {
        self.set_height(lines.len().max(1) as u16)?;
        let text: Vec<Line> = lines.iter().map(|s| ansi::to_line(s)).collect();
        self.term.draw(|f| {
            let area = f.area();
            f.render_widget(Paragraph::new(text), area);
            if let Some((x, y)) = cursor {
                let x = x.min(area.width.saturating_sub(1));
                let y = y.min(area.height.saturating_sub(1));
                f.set_cursor_position(Position::new(area.x + x, area.y + y));
            }
        })?;
        Ok(())
    }

    /// Append lines to the scrollback above the live area (wrapped to the terminal width).
    pub fn print(&mut self, lines: &[String]) -> io::Result<()> {
        if lines.is_empty() {
            return Ok(());
        }
        self.term.autoresize()?;
        let width = self.term.get_frame().area().width as usize;
        let wrapped: Vec<Line> = lines.iter().flat_map(|s| ansi::wrap(ansi::to_line(s), width)).collect();
        for chunk in wrapped.chunks(PRINT_CHUNK) {
            let n = chunk.len() as u16;
            let chunk = chunk.to_vec();
            self.term.insert_before(n, |buf| {
                Paragraph::new(chunk).render(buf.area, buf);
            })?;
        }
        Ok(())
    }

    /// Erase the live area and leave the cursor at its top-left (the next shell prompt goes there).
    pub fn finish(&mut self) {
        let _ = self.term.autoresize();
        let top = self.term.get_frame().area().y;
        let mut out = io::stdout();
        let _ = queue!(out, MoveTo(0, top), Clear(ClearType::FromCursorDown));
        let _ = out.flush();
    }
}

impl Drop for InlineTerm {
    fn drop(&mut self) {
        restore_terminal();
    }
}
