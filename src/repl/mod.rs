//! Interactive mode (`yd` without URLs) on the alternate screen: transcript pane, live area
//! (progress, popup, pickers), composer. A short session summary is printed after leaving it.

mod command;
mod highlight;
mod modals;
mod suggest;
mod view;

use std::collections::VecDeque;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use ratatui::layout::Position;
use unicode_width::UnicodeWidthStr;
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use crossterm::event::{self, Event as TermEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::config::{self, Config};
use crate::format::{DEFAULT_TEMPLATE, FormatSpec};
use crate::tools::{self, ToolEvent, ToolOptions, Tools};
use crate::ui::input::{History, LineEditor};
use crate::ui::picker::{DestPicker, FormatPicker, Interactive, Step};
use crate::ui::term::FullTerm;
use crate::ui::{self, ansi, detail, dim, error_line, heading, ok_line, oneshot, truncate, warn_line};
use crate::ytdlp::{self, Event, Job, Outcome};
use command::{FavArgs, Parsed};

/// Entry point for `yd` without URLs. Returns the process exit code.
pub fn run(config: Config, tool_opts: ToolOptions) -> Result<u8> {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        bail!("interactive mode needs a terminal (pass a URL to download without one)");
    }
    let mut term = FullTerm::new()?;
    let mut app = App::new(config, tool_opts);
    app.print_card();
    let result = app.event_loop(&mut term);
    app.shutdown();
    drop(term); // leave the alternate screen before printing the summary
    app.print_summary();
    result.map(|()| 0)
}

// ---------------------------------------------------------------------------------------------
// Worker
// ---------------------------------------------------------------------------------------------

enum Msg {
    Tool(ToolEvent),
    ToolsReady(Result<(), String>),
    Ev(Event),
    Finished(Result<Outcome, String>),
}

struct QueuedJob {
    job: Job,
    spec: FormatSpec,
    /// One-off overrides shown on the info line (`ogg · best → ~/Music force`).
    note: Option<String>,
}

fn spawn_worker(opts: ToolOptions, cancel: Arc<AtomicBool>, jobs: Receiver<QueuedJob>, tx: Sender<Msg>) {
    std::thread::spawn(move || {
        let tools = tools::ensure_tools(&opts, &mut |e| {
            let _ = tx.send(Msg::Tool(e));
        })
        .map_err(|e| format!("{e:#}"));
        let _ = tx.send(Msg::ToolsReady(tools.as_ref().map(|_| ()).map_err(Clone::clone)));
        for q in jobs {
            let res = match &tools {
                Ok(t) => run_job(&q, t, &cancel, &tx),
                Err(e) => Err(e.clone()),
            };
            let _ = tx.send(Msg::Finished(res));
        }
    });
}

fn run_job(q: &QueuedJob, tools: &Tools, cancel: &AtomicBool, tx: &Sender<Msg>) -> Result<Outcome, String> {
    let built = q.spec.build_args(tools.ffmpeg_dir.is_some())?;
    for w in &built.warnings {
        let _ = tx.send(Msg::Ev(Event::Warning(w.clone())));
    }
    ytdlp::run(&q.job, &built, tools, cancel, |ev| {
        let _ = tx.send(Msg::Ev(ev));
    })
    .map_err(|e| format!("{e:#}"))
}

// ---------------------------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------------------------

struct Running {
    url: String,
    title: String,
    percent: Option<f64>,
    speed: Option<f64>,
    eta: Option<u64>,
    stage: Option<&'static str>,
    started: Instant,
    label: String,
    dest: PathBuf,
    note: Option<String>,
    cancelled: bool,
}

enum Modal {
    Format(FormatPicker),
    Dest(DestPicker),
    Fav(modals::FavState),
    Settings(modals::SettingsState),
}

enum ModalOut {
    Format(FormatSpec),
    Dest(String),
}

impl Modal {
    fn key(&mut self, k: KeyEvent) -> Step<ModalOut> {
        match self {
            Modal::Format(p) => match p.key(k) {
                Step::Done(s) => Step::Done(ModalOut::Format(s)),
                Step::Cancel => Step::Cancel,
                Step::Pending => Step::Pending,
            },
            Modal::Dest(p) => match p.key(k) {
                Step::Done(s) => Step::Done(ModalOut::Dest(s)),
                Step::Cancel => Step::Cancel,
                Step::Pending => Step::Pending,
            },
            // Handled by the App (they edit the config).
            Modal::Fav(_) | Modal::Settings(_) => Step::Pending,
        }
    }

    fn paste(&mut self, s: &str) {
        match self {
            Modal::Format(p) => p.paste(s),
            Modal::Dest(p) => p.paste(s),
            Modal::Fav(_) | Modal::Settings(_) => {}
        }
    }

    fn lines(&self, w: usize) -> Vec<String> {
        match self {
            Modal::Format(p) => p.lines(w),
            Modal::Dest(p) => p.lines(w),
            Modal::Fav(_) | Modal::Settings(_) => Vec::new(),
        }
    }

    fn cursor(&self, w: usize) -> Option<(u16, u16)> {
        match self {
            Modal::Format(p) => p.cursor(w),
            Modal::Dest(p) => p.cursor(w),
            Modal::Fav(_) | Modal::Settings(_) => None,
        }
    }

    fn error(&mut self, msg: String) {
        match self {
            Modal::Format(p) => p.error(msg),
            Modal::Dest(p) => p.error(msg),
            Modal::Fav(_) | Modal::Settings(_) => {}
        }
    }
}

/// Move a list selection by `dir` (+1/-1), wrapping around.
fn move_sel(sel: usize, n: usize, dir: i32) -> usize {
    (sel.min(n - 1) as i32 + dir).rem_euclid(n as i32) as usize
}

const TRANSCRIPT_MAX: usize = 3000;

/// Everything that used to go to scrollback: kept in memory, drawn with wrapping, bottom-anchored.
#[derive(Default)]
struct Transcript {
    lines: Vec<String>,
    ver: u64,
    /// Wrapped lines above the bottom (0 = follow output).
    off: usize,
    /// Output arrived while scrolled up.
    unseen: bool,
    page: usize,
    cache: Option<(usize, u64, Vec<Line<'static>>)>,
    last_total: usize,
    last_w: usize,
}

impl Transcript {
    fn push(&mut self, s: String) {
        self.lines.push(s);
        self.ver += 1;
        if self.lines.len() > TRANSCRIPT_MAX + 500 {
            self.lines.drain(..self.lines.len() - TRANSCRIPT_MAX);
        }
    }

    fn extend(&mut self, it: impl IntoIterator<Item = String>) {
        for l in it {
            self.push(l);
        }
    }

    /// The visible window of the document `transcript + live` (top-aligned; the tail when it is
    /// taller than `h`). `cursor` is (col, row) inside `live`; the result is relative to the window.
    fn visible(
        &mut self,
        w: usize,
        h: usize,
        live: Vec<Line<'static>>,
        cursor: Option<(u16, u16)>,
    ) -> (Vec<Line<'static>>, Option<(u16, u16)>) {
        if self.cache.as_ref().is_none_or(|(cw, cv, _)| *cw != w || *cv != self.ver) {
            let wrapped = self.lines.iter().flat_map(|l| ansi::wrap(ansi::to_line(l), w)).collect();
            self.cache = Some((w, self.ver, wrapped));
        }
        let all = &self.cache.as_ref().unwrap().2;
        let t = all.len();
        let total = t + live.len();
        if self.off > 0 && self.last_w == w && total > self.last_total {
            self.off += total - self.last_total;
            self.unseen = true;
        }
        self.last_total = total;
        self.last_w = w;
        self.page = h.saturating_sub(2).max(1);
        self.off = self.off.min(total.saturating_sub(h));
        if self.off == 0 {
            self.unseen = false;
        }
        let end = total - self.off;
        let start = end.saturating_sub(h);
        let rows = (start..end)
            .map(|i| if i < t { all[i].clone() } else { live[i - t].clone() })
            .collect();
        let cur = cursor.and_then(|(x, y)| {
            let row = t + y as usize;
            (row >= start && row < end).then(|| (x, (row - start) as u16))
        });
        (rows, cur)
    }

    fn up(&mut self, n: usize) {
        self.off = self.off.saturating_add(n);
    }

    fn down(&mut self, n: usize) {
        self.off = self.off.saturating_sub(n);
    }
}

struct App {
    cfg: Config,
    spec: FormatSpec,
    dest: PathBuf,
    composer: LineEditor,
    history: History,
    popup_sel: usize,
    popup_dismissed: bool,
    modal: Option<Modal>,
    /// Settings screen waiting while a picker opened from it is active.
    parent: Option<Modal>,
    /// Terminal height of the last frame.
    height: usize,
    queue: VecDeque<QueuedJob>,
    current: Option<Running>,
    /// Tool download in progress: (name, done, total).
    tool: Option<(&'static str, u64, Option<u64>)>,
    tool_header: bool,
    playlist: Option<String>,
    job_tx: Sender<QueuedJob>,
    rx: Receiver<Msg>,
    cancel: Arc<AtomicBool>,
    /// Lines waiting to be pushed into the scrollback.
    scroll: Transcript,
    /// Session summary data.
    saved: Vec<PathBuf>,
    skipped: usize,
    failed: Vec<String>,
    quit: bool,
    started: Instant,
}

impl App {
    fn new(cfg: Config, opts: ToolOptions) -> Self {
        let spec = cfg
            .last_format
            .as_deref()
            .and_then(|s| FormatSpec::parse(s).ok())
            .unwrap_or(FormatSpec::DEFAULT);
        let dest = cfg.default_dest();
        let mut initial_history = if cfg.settings.history_persist { config::load_history() } else { Vec::new() };
        let keep = cfg.settings.history_keep;
        initial_history.drain(..initial_history.len().saturating_sub(keep));
        let cancel = Arc::new(AtomicBool::new(false));
        let (job_tx, job_rx) = mpsc::channel();
        let (tx, rx) = mpsc::channel();
        spawn_worker(opts, cancel.clone(), job_rx, tx);
        App {
            cfg,
            spec,
            dest,
            composer: LineEditor::default(),
            history: History::from_items(initial_history),
            popup_sel: 0,
            popup_dismissed: false,
            modal: None,
            parent: None,
            height: 24,
            queue: VecDeque::new(),
            current: None,
            tool: None,
            tool_header: false,
            playlist: None,
            job_tx,
            rx,
            cancel,
            scroll: Transcript::default(),
            saved: Vec::new(),
            skipped: 0,
            failed: Vec::new(),
            quit: false,
            started: Instant::now(),
        }
    }

    fn push(&mut self, line: impl Into<String>) {
        self.scroll.push(line.into());
    }

    fn fav_name_for(&self, dir: &Path) -> Option<&str> {
        self.cfg.favorites.iter().find(|f| f.path == dir).map(|f| f.name.as_str())
    }

    fn print_card(&mut self) {
        let width = crossterm::terminal::size().map(|(w, _)| w as usize).unwrap_or(80);
        let lines = view::card(width, &self.spec, &self.dest, self.fav_name_for(&self.dest));
        self.scroll.extend(lines);
    }

    fn save_cfg(&mut self) {
        if let Err(e) = self.cfg.save() {
            self.push(warn_line(&format!("cannot save config: {e:#}")));
        }
    }

    // ----- main loop ---------------------------------------------------------------------

    fn event_loop(&mut self, term: &mut FullTerm) -> Result<()> {
        while !self.quit {
            let width = crossterm::terminal::size().map(|(w, _)| w as usize).unwrap_or(80);
            while let Ok(msg) = self.rx.try_recv() {
                self.on_msg(msg, width);
            }
            term.draw(|f| self.draw(f))?;

            let animating = self.current.is_some() || self.tool.is_some();
            let timeout = Duration::from_millis(if animating { 60 } else { 200 });
            if event::poll(timeout)? {
                loop {
                    match event::read()? {
                        TermEvent::Key(k) if k.kind != KeyEventKind::Release => self.on_key(k),
                        TermEvent::Paste(s) => self.on_paste(&s),
                        _ => {}
                    }
                    if self.quit || !event::poll(Duration::ZERO)? {
                        break;
                    }
                }
            }
        }
        Ok(())
    }

    /// Stop a running download (so no yt-dlp is left behind) before the terminal is restored.
    fn shutdown(&mut self) {
        self.cancel.store(true, Ordering::SeqCst);
        self.queue.clear();
        let deadline = Instant::now() + Duration::from_secs(3);
        while self.current.is_some() {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            match self.rx.recv_timeout(left) {
                Ok(Msg::Finished(_)) => self.current = None,
                Ok(_) => {}
                Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => break,
            }
        }
    }

    // ----- worker messages ---------------------------------------------------------------

    fn on_msg(&mut self, msg: Msg, width: usize) {
        match msg {
            Msg::Tool(ev) => match ev {
                ToolEvent::Found { .. } => {}
                ToolEvent::Downloading { name, done, total } => {
                    self.tool_header();
                    self.tool = Some((name, done, total));
                }
                ToolEvent::Installed { name, path } => {
                    self.tool = None;
                    self.tool_header();
                    self.push(detail(&format!("{name}  {}", ui::abbrev_home(&path))));
                }
                ToolEvent::Warn(m) => self.push(warn_line(&m)),
            },
            Msg::ToolsReady(r) => {
                self.tool = None;
                if let Err(e) = r {
                    self.push(error_line(&format!("tool setup failed: {e}")));
                }
            }
            Msg::Ev(ev) => self.on_download_event(ev, width),
            Msg::Finished(res) => self.on_finished(res),
        }
    }

    fn tool_header(&mut self) {
        if !self.tool_header {
            self.tool_header = true;
            self.push(heading("Setting up tools"));
        }
    }

    fn on_download_event(&mut self, ev: Event, width: usize) {
        let Some(run) = self.current.as_mut() else { return };
        if run.cancelled {
            return;
        }
        match ev {
            Event::Info(info) => {
                run.started = Instant::now();
                run.percent = None;
                run.stage = None;
                run.title = match (info.playlist_index, info.n_entries) {
                    (Some(i), Some(n)) if n > 1 => format!("[{i}/{n}] {}", info.title),
                    _ => info.title.clone(),
                };
                let mut lines = oneshot::info_lines(&info, &mut self.playlist);
                if let Some(note) = &run.note {
                    match lines.last_mut() {
                        Some(l) if l.contains(ui::BRANCH) => l.push_str(&dim(format!(" \u{b7} {note}"))),
                        _ => lines.push(detail(note)),
                    }
                }
                self.scroll.extend(lines);
            }
            Event::Progress {
                percent, speed, eta, ..
            } => {
                run.percent = Some(percent);
                run.speed = speed;
                run.eta = eta;
                run.stage = None;
            }
            Event::PostProcess(name) => {
                if let Some(label) = oneshot::stage_label(&name) {
                    run.stage = Some(label);
                }
            }
            Event::Done { path, size } => {
                let lines = oneshot::done_lines(&path, size, &run.label, run.started.elapsed().as_secs());
                run.percent = None;
                run.stage = None;
                self.scroll.extend(lines);
                self.saved.push(path);
            }
            Event::AlreadyDownloaded { path } => {
                run.percent = None;
                self.skipped += 1;
                self.scroll.extend(oneshot::already_lines(&path));
            }
            Event::Warning(m) => self.scroll.push(warn_line(&truncate(&m, width.saturating_sub(2)))),
            Event::Error(m) => {
                self.failed.push(format!("{} \u{b7} {}", truncate(&run.url, 40), truncate(&m, 80)));
                self.scroll.push(error_line(&m));
            }
        }
    }

    fn on_finished(&mut self, res: Result<Outcome, String>) {
        let run = self.current.take();
        self.cancel.store(false, Ordering::SeqCst);
        let cancelled = run.as_ref().is_some_and(|r| r.cancelled);
        match res {
            Ok(o) => {
                if !cancelled
                    && !o.cancelled
                    && (o.success() || !o.saved.is_empty() || !o.already.is_empty())
                    && let Some(r) = &run
                {
                    self.cfg.push_recent(&r.dest);
                    self.save_cfg();
                }
            }
            Err(e) if !cancelled => {
                let url = run.as_ref().map(|r| truncate(&r.url, 40)).unwrap_or_default();
                self.failed.push(format!("{url} \u{b7} {}", truncate(&e, 80)));
                self.push(error_line(&e));
            }
            Err(_) => {}
        }
        if !cancelled {
            self.push(String::new());
        }
        self.pump();
    }

    // ----- queue -------------------------------------------------------------------------

    fn enqueue(&mut self, url: &str) {
        self.enqueue_with(url, self.spec, self.dest.clone(), false);
    }

    fn enqueue_with(&mut self, url: &str, spec: FormatSpec, dest: PathBuf, force: bool) {
        let mut note = Vec::new();
        if spec != self.spec {
            note.push(spec.to_string());
        }
        if dest != self.dest {
            note.push(format!("\u{2192} {}", ui::abbrev_home(&dest)));
        }
        if force {
            note.push("force".to_string());
        }
        let note = (!note.is_empty()).then(|| note.join(" "));
        let job = Job {
            url: url.to_string(),
            dest,
            template: self.cfg.template.clone().unwrap_or_else(|| DEFAULT_TEMPLATE.to_string()),
            force: force || self.cfg.settings.force,
            playlist: self.cfg.settings.playlist,
            cookies_from_browser: self.cfg.settings.cookies.clone(),
            extra: Vec::new(),
        };
        self.queue.push_back(QueuedJob { job, spec, note });
        self.pump();
    }

    /// Hand the next queued job to the worker when it is idle.
    fn pump(&mut self) {
        if self.current.is_some() {
            return;
        }
        let Some(q) = self.queue.pop_front() else { return };
        self.current = Some(Running {
            url: q.job.url.clone(),
            title: q.job.url.clone(),
            percent: None,
            speed: None,
            eta: None,
            stage: None,
            started: Instant::now(),
            label: q.spec.to_string(),
            dest: q.job.dest.clone(),
            note: q.note.clone(),
            cancelled: false,
        });
        let _ = self.job_tx.send(q);
    }

    /// Ctrl+C while downloading: kill the current download and drop the queue.
    fn cancel_all(&mut self) {
        self.queue.clear();
        if let Some(run) = self.current.as_mut()
            && !run.cancelled
        {
            run.cancelled = true;
            self.cancel.store(true, Ordering::SeqCst);
            self.scroll.push(heading("Cancelled"));
            self.scroll.push(String::new());
        }
    }

    // ----- input -------------------------------------------------------------------------

    /// Candidates for the token being typed (only when the cursor is at the end of the line).
    fn suggestions(&self) -> Option<suggest::Suggestions> {
        let text = self.composer.text();
        if self.popup_dismissed || self.modal.is_some() || text.is_empty() || self.composer.cursor() != text.len() {
            return None;
        }
        let ctx = suggest::Ctx {
            favorites: &self.cfg.favorites,
            recent: &self.cfg.recent,
        };
        let sug = suggest::suggest(text, &ctx);
        if sug.items.is_empty() {
            return None;
        }
        let tok = &text[sug.start..];
        // A single, already complete candidate has nothing left to offer.
        if sug.items.len() == 1 && suggest::is_complete(tok, &sug.items[0]) {
            return None;
        }
        Some(sug)
    }

    /// Replace the token being typed by the selected candidate.
    fn accept_suggestion(&mut self, sug: &suggest::Suggestions) {
        let c = &sug.items[self.popup_sel.min(sug.items.len() - 1)];
        let text = format!("{}{}", &self.composer.text()[..sug.start], c.insert);
        self.composer.set(&text);
        self.popup_sel = 0;
    }

    /// Echo of a submitted line, highlighted like the composer (dim).
    fn echo(&mut self, text: &str) {
        let segs = highlight::segments(text, &self.cfg);
        self.push(format!("{} {}", dim("\u{203a}"), highlight::render(&segs, true)));
    }

    fn persist_history(&mut self) {
        if self.cfg.settings.history_persist {
            let keep = self.cfg.settings.history_keep;
            self.history.trim_to(keep);
            if let Err(e) = config::save_history(self.history.items()) {
                self.push(warn_line(&format!("cannot save history: {e:#}")));
            }
        }
    }

    fn on_paste(&mut self, s: &str) {
        if let Some(m) = self.modal.as_mut() {
            m.paste(s);
            return;
        }
        // Several URLs at once: enqueue them all without waiting for Enter.
        if s.split_whitespace().count() > 1
            && let Some(urls) = command::extract_urls(s)
        {
            for u in urls {
                self.push(dim(format!("\u{203a} {u}")));
                self.enqueue(&u);
            }
            return;
        }
        self.composer.insert_str(s);
        self.popup_dismissed = false;
    }

    fn on_key(&mut self, k: KeyEvent) {
        match self.modal.take() {
            Some(Modal::Fav(st)) => return self.fav_key(st, k),
            Some(Modal::Settings(st)) => return self.settings_key(st, k),
            other => self.modal = other,
        }
        if let Some(m) = self.modal.as_mut() {
            match m.key(k) {
                Step::Pending => {}
                Step::Cancel => self.modal = self.parent.take(),
                Step::Done(out) => self.on_modal_done(out),
            }
            return;
        }
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let shift = k.modifiers.contains(KeyModifiers::SHIFT);
        let sug = self.suggestions();
        if k.code != KeyCode::Esc {
            self.popup_dismissed = false;
        }
        match k.code {
            KeyCode::PageUp => self.scroll.up(self.scroll.page),
            KeyCode::PageDown => self.scroll.down(self.scroll.page),
            KeyCode::Char('p') if ctrl => self.history_prev(),
            KeyCode::Char('n') if ctrl => self.history_next(),
            KeyCode::End if self.scroll.off > 0 => self.scroll.off = 0,
            KeyCode::Char('c') if ctrl => {
                if self.current.is_some() {
                    self.cancel_all();
                } else if self.composer.is_empty() {
                    self.quit = true;
                } else {
                    self.composer.clear();
                }
            }
            KeyCode::Char('d') if ctrl && self.composer.is_empty() => self.quit = true,
            KeyCode::Enter => {
                // Enter completes a partial token first; it submits once the token is complete
                // (or when the cursor sits after a space: `/o ` opens the picker).
                if let Some(sug) = &sug {
                    let tok = &self.composer.text()[sug.start..];
                    let c = &sug.items[self.popup_sel.min(sug.items.len() - 1)];
                    if !tok.is_empty() && !suggest::is_complete(tok, c) {
                        self.accept_suggestion(sug);
                        return;
                    }
                }
                self.submit();
            }
            KeyCode::Esc => self.popup_dismissed = true,
            KeyCode::Tab | KeyCode::Right if sug.is_some() && self.composer.cursor() == self.composer.text().len() => {
                if let Some(sug) = &sug {
                    self.accept_suggestion(sug);
                }
            }
            // Shift+↑↓ choose a candidate; plain ↑↓ (and the mouse wheel) stay input history.
            KeyCode::Up | KeyCode::Down if shift => {
                if let Some(sug) = &sug {
                    let dir = if k.code == KeyCode::Up { -1 } else { 1 };
                    self.popup_sel = move_sel(self.popup_sel, sug.items.len(), dir);
                }
            }
            KeyCode::Up => self.history_prev(),
            KeyCode::Down => self.history_next(),
            _ => {
                let before = self.composer.text().to_string();
                if self.composer.handle_key(&k) && self.composer.text() != before {
                    self.popup_sel = 0;
                    self.history.reset();
                }
            }
        }
    }

    fn history_prev(&mut self) {
        if let Some(t) = self.history.prev(self.composer.text()) {
            self.composer.set(&t);
        }
    }

    fn history_next(&mut self) {
        if let Some(t) = self.history.next() {
            self.composer.set(&t);
        }
    }

    fn submit(&mut self) {
        let text = self.composer.text().trim().to_string();
        if text.is_empty() {
            return;
        }
        self.composer.clear();
        self.popup_sel = 0;
        self.history.push(&text);
        self.persist_history();
        match command::parse_input(&text) {
            Parsed::Empty => {}
            Parsed::Urls(urls) => {
                for u in urls {
                    self.push(dim(format!("\u{203a} {u}")));
                    self.enqueue(&u);
                }
            }
            Parsed::Inline(Err(msg)) => {
                self.echo(&text);
                self.push(error_line(&msg));
                self.push(detail("usage: URL... [/f format] [/o folder] [/force]"));
            }
            Parsed::Inline(Ok(inl)) => {
                self.echo(&text);
                self.enqueue_inline(inl);
            }
            Parsed::Command { cmd, args } => {
                self.echo(&text);
                self.run_command(cmd.name, &args);
            }
            Parsed::UnknownCommand(w) => {
                self.echo(&text);
                self.push(error_line(&format!("unknown command /{w}")));
                self.push(detail("type /help for the list of commands"));
            }
            Parsed::NotUrl(tok) => {
                self.echo(&text);
                self.push(error_line(&format!("not a URL: {}", truncate(&tok, 40))));
                self.push(detail("paste a URL, or /help for commands"));
            }
        }
    }

    /// Validate every override first; enqueue nothing when any is bad.
    fn enqueue_inline(&mut self, inl: command::Inline) {
        let spec = match inl.format.as_deref().map(FormatSpec::parse) {
            Some(Ok(s)) => s,
            Some(Err(e)) => return self.push(error_line(&e)),
            None => self.spec,
        };
        let dest = match inl.dest.as_deref().map(|d| self.cfg.resolve_dest(d)) {
            Some(Ok(d)) => d.components().collect(),
            Some(Err(e)) => return self.push(error_line(&format!("{e:#}"))),
            None => self.dest.clone(),
        };
        for u in &inl.urls {
            self.enqueue_with(u, spec, PathBuf::clone(&dest), inl.force);
        }
    }

    // ----- commands ----------------------------------------------------------------------

    fn run_command(&mut self, name: &str, args: &str) {
        match name {
            "f" if args.is_empty() => self.modal = Some(Modal::Format(FormatPicker::new(&self.spec))),
            "f" => match FormatSpec::parse(args) {
                Ok(s) => self.set_format(s),
                Err(e) => self.push(error_line(&e)),
            },
            "o" if args.is_empty() => {
                self.modal = Some(Modal::Dest(DestPicker::new(&self.cfg, &self.dest)));
            }
            "o" => match self.cfg.resolve_dest(args) {
                Ok(d) => self.set_dest(d),
                Err(e) => self.push(error_line(&format!("{e:#}"))),
            },
            "fav" if args.is_empty() => self.open_fav(),
            "fav" => self.run_fav(args),
            "settings" => self.open_settings(),
            "open" => {
                let dest = self.dest.clone();
                let _ = std::fs::create_dir_all(&dest);
                match command::open_folder(&dest) {
                    Ok(()) => self.push(ok_line(&format!("Opening {}", ui::abbrev_home(&dest)))),
                    Err(e) => self.push(error_line(&format!("cannot open folder: {e}"))),
                }
            }
            "help" => self.print_help(),
            "q" => self.quit = true,
            _ => {}
        }
    }

    fn run_fav(&mut self, args: &str) {
        match command::parse_fav(args) {
            FavArgs::List => {
                let lines = ui::fav_list_lines(&self.cfg);
                self.scroll.extend(lines);
            }
            FavArgs::Add(name) => match self.cfg.add_favorite(&name, &self.dest.display().to_string()) {
                Ok(p) => {
                    self.save_cfg();
                    self.push(ok_line(&format!("Favorite {name} {}", ui::abbrev_home(&p))));
                }
                Err(e) => self.push(error_line(&format!("{e:#}"))),
            },
            FavArgs::Rm(name) => {
                if self.cfg.remove_favorite(&name) {
                    self.save_cfg();
                    self.push(ok_line(&format!("Removed favorite {name}")));
                } else {
                    self.push(error_line(&format!("no favorite named '{name}'")));
                }
            }
            FavArgs::Bad(msg) => self.push(error_line(msg)),
        }
    }

    fn print_help(&mut self) {
        self.push(heading("Commands"));
        for c in &command::COMMANDS {
            let args = if c.args.is_empty() { String::new() } else { format!(" {}", c.args) };
            self.push(detail(&format!("{}{args}  {}", c.label(), c.help)));
        }
        self.push(heading("Keys"));
        self.push(detail("\u{23ce} download (several URLs may be pasted at once; they queue)"));
        self.push(detail("\u{2191}\u{2193} / ctrl+p/n history \u{b7} PgUp/PgDn scroll \u{b7} End follow \u{b7} shift+\u{2191}\u{2193} choose suggestion \u{b7} tab or \u{2192} accept"));
        self.push(detail("ctrl+u clear \u{b7} ctrl+w delete word"));
        self.push(detail("ctrl+c cancel download / quit \u{b7} ctrl+d quit"));
    }

    fn set_format(&mut self, spec: FormatSpec) {
        self.spec = spec;
        self.cfg.last_format = Some(spec.canonical());
        self.save_cfg();
        self.push(ok_line(&format!("Format {spec}")));
    }

    /// Make `dir` the persistent save folder (no transcript output).
    fn apply_dest(&mut self, dir: PathBuf) -> PathBuf {
        // Drop a trailing slash typed by the user so paths compare equal.
        let dir: PathBuf = dir.components().collect();
        self.cfg.last_dest = Some(dir.clone());
        self.cfg.push_recent(&dir);
        self.save_cfg();
        self.dest = dir.clone();
        dir
    }

    fn set_dest(&mut self, dir: PathBuf) {
        let dir = self.apply_dest(dir);
        let fav = self.fav_name_for(&dir).map(|n| format!("  \u{2605} {n}")).unwrap_or_default();
        self.push(ok_line(&format!("Save to {}{fav}", ui::abbrev_home(&dir))));
    }

    fn on_modal_done(&mut self, out: ModalOut) {
        match out {
            ModalOut::Format(s) => {
                self.modal = self.parent.take();
                self.set_format(s);
            }
            ModalOut::Dest(s) => match self.cfg.resolve_dest(&s) {
                Ok(d) => {
                    self.modal = self.parent.take();
                    self.set_dest(d);
                }
                Err(e) => {
                    if let Some(m) = self.modal.as_mut() {
                        m.error(format!("{e:#}"));
                    }
                }
            },
        }
    }

    // ----- view --------------------------------------------------------------------------

    fn progress(&self, width: usize) -> Option<String> {
        let tick = (self.started.elapsed().as_millis() / 100) as usize;
        if let Some((name, done, total)) = self.tool {
            let pct = total.filter(|t| *t > 0).map(|t| done as f64 / t as f64 * 100.0);
            return Some(view::progress_line(
                &view::Progress {
                    tick,
                    title: &format!("Downloading {name}"),
                    percent: pct,
                    speed: None,
                    eta: None,
                    stage: None,
                    queued: self.queue.len(),
                },
                width,
            ));
        }
        let run = self.current.as_ref()?;
        if run.cancelled {
            return Some(format!("  {} {}", ui::spinner_frame(tick), dim("Cancelling\u{2026}")));
        }
        Some(view::progress_line(
            &view::Progress {
                tick,
                title: &run.title,
                percent: run.percent,
                speed: run.speed,
                eta: run.eta,
                stage: run.stage,
                queued: self.queue.len(),
            },
            width,
        ))
    }

    /// Draw one frame: one flowing document (card, transcript, live area, composer, footer),
    /// top-aligned; once taller than the screen the tail is shown.
    fn draw(&mut self, f: &mut ratatui::Frame) {
        let area = f.area();
        let (w, h) = (area.width as usize, area.height as usize);
        self.height = h;
        let (live, cursor) = self.view(w);
        let live: Vec<Line> = live.iter().map(|l| ansi::to_line(l)).collect();
        let (rows, cur) = self.scroll.visible(w, h, live, cursor);
        f.render_widget(Paragraph::new(rows), area);
        if let Some((x, y)) = cur {
            f.set_cursor_position(Position::new(x.min(area.width.saturating_sub(1)), y));
        }
    }

    fn view(&self, width: usize) -> (Vec<String>, Option<(u16, u16)>) {
        let mut lines = Vec::new();
        if let Some(p) = self.progress(width) {
            lines.push(p);
        }
        if let Some(m) = &self.modal {
            let (body, cur) = match m {
                Modal::Fav(st) => self.fav_lines(st, width),
                Modal::Settings(st) => self.settings_lines(st, width),
                other => (other.lines(width), other.cursor(width)),
            };
            let cursor = cur.map(|(x, y)| (x, y + lines.len() as u16));
            lines.extend(body);
            return (lines, cursor);
        }
        let sug = self.suggestions();
        if let Some(sug) = &sug {
            lines.extend(view::suggest_lines(&sug.items, self.popup_sel.min(sug.items.len() - 1), width));
        }
        let row = lines.len() as u16;
        let avail = width.saturating_sub(3);
        let (start, end, col) = self.composer.window_range(avail);
        let prompt = ui::cyan("\u{203a}");
        if self.composer.is_empty() {
            lines.push(format!("{prompt} {}", dim("Paste a URL\u{2026}")));
        } else {
            let segs = highlight::segments(self.composer.text(), &self.cfg);
            let mut body = highlight::render(&highlight::clip(&segs, start, end), false);
            if let Some(sug) = &sug {
                let ghost = suggest::ghost(self.composer.text(), sug, self.popup_sel.min(sug.items.len() - 1));
                let shown_w = self.composer.text()[start..end].width();
                if end == self.composer.text().len() && shown_w + ghost.width() < avail {
                    body.push_str(&dim(ghost));
                }
            }
            lines.push(format!("{prompt} {body}"));
        }
        let tail = if self.current.is_some() { "ctrl+c cancel" } else { "ctrl+c quit" };
        let footer = if sug.is_some() {
            "  shift+\u{2191}\u{2193} choose \u{b7} tab accept \u{b7} \u{23ce} complete / run \u{b7} esc hide".to_string()
        } else if self.scroll.off > 0 {
            let what = if self.scroll.unseen { "new output" } else { "scrolled" };
            format!("  \u{2193} {what} \u{b7} PgDn / End to follow")
        } else {
            format!("  \u{23ce} download \u{b7} \u{2191} history \u{b7} /f format \u{b7} /o save to \u{b7} /fav \u{b7} /settings \u{b7} {tail}")
        };
        lines.push(dim(truncate(&footer, width.saturating_sub(1))));
        (lines, Some((2 + col as u16, row)))
    }
}

const SUMMARY_MAX_FILES: usize = 20;

impl App {
    /// Codex-style recap printed to the normal screen after leaving the alternate screen.
    fn summary_lines(&self) -> Vec<String> {
        if self.saved.is_empty() && self.skipped == 0 && self.failed.is_empty() {
            return Vec::new();
        }
        let mut parts = Vec::new();
        if !self.saved.is_empty() {
            parts.push(format!("{} saved", self.saved.len()));
        }
        if self.skipped > 0 {
            parts.push(format!("{} skipped", self.skipped));
        }
        if !self.failed.is_empty() {
            parts.push(format!("{} failed", self.failed.len()));
        }
        let mut out = vec![heading(&format!("yd session \u{b7} {}", parts.join(" \u{b7} ")))];
        for p in self.saved.iter().take(SUMMARY_MAX_FILES) {
            out.push(detail(&ui::abbrev_home(p)));
        }
        if self.saved.len() > SUMMARY_MAX_FILES {
            out.push(detail(&format!("\u{2026} and {} more", self.saved.len() - SUMMARY_MAX_FILES)));
        }
        for f in &self.failed {
            out.push(format!("  {} {f}", ui::red(ui::CROSS)));
        }
        out
    }

    fn print_summary(&self) {
        if !self.cfg.settings.exit_summary {
            return;
        }
        for l in self.summary_lines() {
            println!("{l}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> App {
        App::new(Config::default(), ToolOptions { skip_deno: true })
    }

    #[test]
    fn summary_lists_saved_and_failures() {
        let mut a = app();
        assert!(a.summary_lines().is_empty());
        a.saved = (0..25).map(|i| PathBuf::from(format!("/m/{i}.ogg"))).collect();
        a.skipped = 1;
        a.failed.push("url \u{b7} boom".into());
        let l = a.summary_lines();
        assert!(ansi::to_line(&l[0]).spans.iter().map(|s| s.content.as_ref()).collect::<String>().contains("25 saved \u{b7} 1 skipped \u{b7} 1 failed"));
        assert_eq!(l.len(), 1 + 20 + 1 + 1);
        assert!(l[21].contains("5 more"));
    }

    #[test]
    fn selection_wraps() {
        assert_eq!(move_sel(0, 3, -1), 2);
        assert_eq!(move_sel(2, 3, 1), 0);
        assert_eq!(move_sel(5, 3, 1), 0);
        assert_eq!(move_sel(0, 1, 1), 0);
    }

    #[test]
    fn document_flows_top_aligned_and_follows() {
        let live = || vec![Line::from("composer"), Line::from("footer")];
        let mut t = Transcript::default();
        for i in 0..3 {
            t.push(format!("line {i}"));
        }
        // Short document: starts at row 0, the composer sits right after the content.
        let (rows, cur) = t.visible(40, 10, live(), Some((2, 0)));
        assert_eq!(rows.len(), 5);
        assert_eq!(cur, Some((2, 3)));
        for i in 3..50 {
            t.push(format!("line {i}"));
        }
        // Tall document: the tail is shown, the composer ends up near the bottom.
        let (rows, cur) = t.visible(40, 10, live(), Some((2, 0)));
        assert_eq!(rows.len(), 10);
        assert!(rows[7].to_string().contains("line 49"));
        assert_eq!(cur, Some((2, 8)));
        t.up(5);
        let (rows, cur) = t.visible(40, 10, live(), Some((2, 0)));
        assert!(rows[9].to_string().contains("line 46") && cur.is_none());
        t.push("line 50".into());
        let (rows, _) = t.visible(40, 10, live(), None);
        assert!(rows[9].to_string().contains("line 46") && t.unseen);
        t.up(1000);
        t.visible(40, 10, live(), None);
        assert_eq!(t.off, 53 - 10);
        t.down(1000);
        let (rows, _) = t.visible(40, 10, live(), None);
        assert!(rows[9].to_string().contains("footer") && !t.unseen);
        let mut t = Transcript::default();
        for i in 0..4000 {
            t.push(format!("{i}"));
        }
        assert!(t.lines.len() <= TRANSCRIPT_MAX + 500);
    }
}
