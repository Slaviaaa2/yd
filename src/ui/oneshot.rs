//! One-shot output renderer: consumes yt-dlp / tool events, prints styled status lines.

use std::io::Write;
use std::time::{Duration, Instant};

use super::*;
use crate::tools::ToolEvent;
use crate::ytdlp::{Event, Info};

const REDRAW_EVERY: Duration = Duration::from_millis(100);
pub const BAR_WIDTH: usize = 20;

fn flush() {
    let _ = std::io::stdout().flush();
}

/// Single live line redrawn in place (TTY only).
struct Live {
    shown: bool,
    last: Option<Instant>,
    tick: usize,
}

impl Live {
    fn new() -> Self {
        Live {
            shown: false,
            last: None,
            tick: 0,
        }
    }

    fn clear(&mut self) {
        if self.shown {
            print!("\r\x1b[2K");
            flush();
            self.shown = false;
        }
    }

    /// Draw `line` (already styled); `plain_len` is its display width for truncation safety.
    fn draw(&mut self, line: &str, force: bool) {
        if !is_tty() {
            return;
        }
        if !force && self.last.is_some_and(|t| t.elapsed() < REDRAW_EVERY) {
            return;
        }
        self.last = Some(Instant::now());
        self.tick += 1;
        print!("\r\x1b[2K{line}");
        flush();
        self.shown = true;
    }
}

pub fn stage_label(pp: &str) -> Option<&'static str> {
    Some(match pp {
        "ExtractAudio" => "Converting audio",
        "Merger" => "Merging streams",
        "VideoConvertor" => "Converting video",
        "VideoRemuxer" => "Remuxing",
        "EmbedThumbnail" => "Embedding thumbnail",
        "FFmpegMetadata" | "Metadata" => "Writing metadata",
        "FFmpegFixupM4a" | "FixupM4a" | "FixupM3u8" => "Fixing up",
        "MoveFiles" => return None,
        _ => "Processing",
    })
}

pub struct Printer {
    format_label: String,
    live: Live,
    started: Instant,
    playlist: Option<String>,
}

impl Printer {
    pub fn new(format_label: impl Into<String>) -> Self {
        Printer {
            format_label: format_label.into(),
            live: Live::new(),
            started: Instant::now(),
            playlist: None,
        }
    }

    pub fn event(&mut self, ev: Event) {
        match ev {
            Event::Info(info) => {
                self.live.clear();
                self.started = Instant::now();
                self.print_info(&info);
                self.live
                    .draw(&format!("  {} {}", spinner_frame(0), dim("Starting")), true);
            }
            Event::Progress {
                percent, speed, eta, ..
            } => {
                let mut parts = Vec::new();
                if let Some(s) = speed {
                    parts.push(fmt_speed(s));
                }
                if let Some(e) = eta {
                    parts.push(fmt_duration(e as f64));
                }
                let tail = if parts.is_empty() {
                    String::new()
                } else {
                    format!("  {}", dim(parts.join(" \u{b7} ")))
                };
                let line = format!(
                    "  {} {:>3}% {}{}",
                    spinner_frame(self.live.tick),
                    percent as u32,
                    progress_bar(percent, BAR_WIDTH),
                    tail
                );
                self.live.draw(&line, false);
            }
            Event::PostProcess(name) => {
                if let Some(label) = stage_label(&name) {
                    let line = format!(
                        "  {} {}",
                        spinner_frame(self.live.tick),
                        dim(format!("{label}\u{2026}"))
                    );
                    self.live.draw(&line, true);
                }
            }
            Event::Done { path, size } => {
                self.live.clear();
                for l in done_lines(&path, size, &self.format_label, self.started.elapsed().as_secs()) {
                    println!("{l}");
                }
                flush();
            }
            Event::AlreadyDownloaded { path } => {
                self.live.clear();
                for l in already_lines(&path) {
                    println!("{l}");
                }
                flush();
            }
            Event::Uploading => {
                self.live.clear();
                println!("{}", heading("Uploading to temp.sh"));
                flush();
            }
            Event::Uploaded(link) => {
                println!("{}", ok_line(&format!("Uploaded {link}")));
                flush();
            }
            Event::UploadRetained(path) => {
                println!("{}", warn_line(&format!("Files kept in {}", abbrev_home(&path))));
                flush();
            }
            Event::Warning(msg) => {
                self.live.clear();
                println!("{}", warn_line(&truncate(&msg, term_width().saturating_sub(2))));
                flush();
            }
            Event::Error(msg) => {
                self.live.clear();
                flush();
                eprintln!("{}", error_line(&msg));
            }
        }
    }

    /// Clear the live line (call when finishing or on cancel).
    pub fn finish(&mut self) {
        self.live.clear();
    }

    fn print_info(&mut self, info: &Info) {
        for l in info_lines(info, &mut self.playlist) {
            println!("{l}");
        }
        flush();
    }
}

/// Lines announcing a download (`• Title` + `└ uploader · 0:19 · Youtube`, or a playlist entry).
/// `playlist` remembers the last playlist heading so it is printed once.
pub fn info_lines(info: &Info, playlist: &mut Option<String>) -> Vec<String> {
    let mut out = Vec::new();
    let in_list = info.n_entries.is_some_and(|n| n > 1) && info.playlist_title.is_some();
    if in_list {
        let pt = info.playlist_title.clone().unwrap_or_default();
        if playlist.as_deref() != Some(pt.as_str()) {
            out.push(heading(&format!("{pt} ({} items)", info.n_entries.unwrap_or(0))));
            *playlist = Some(pt);
        }
        let idx = info
            .playlist_index
            .map(|i| format!("[{i}/{}] ", info.n_entries.unwrap_or(0)))
            .unwrap_or_default();
        out.push(format!("  {}{}", dim(idx), info.title));
    } else {
        out.push(heading(&info.title));
        let mut d = Vec::new();
        if let Some(u) = &info.uploader {
            d.push(u.clone());
        }
        if let Some(s) = info.duration {
            d.push(fmt_duration(s));
        }
        if let Some(e) = &info.extractor {
            d.push(e.clone());
        }
        if !d.is_empty() {
            out.push(detail(&d.join(" \u{b7} ")));
        }
    }
    out
}

/// `✔ Saved <path>` + `└ size · format · time`.
pub fn done_lines(path: &Path, size: Option<u64>, format_label: &str, elapsed_secs: u64) -> Vec<String> {
    let mut d = Vec::new();
    if let Some(s) = size {
        d.push(fmt_bytes(s));
    }
    d.push(format_label.to_string());
    d.push(fmt_elapsed(elapsed_secs));
    vec![
        ok_line(&format!("Saved {}", abbrev_home(path))),
        detail(&d.join(" \u{b7} ")),
    ]
}

pub fn already_lines(path: &Path) -> Vec<String> {
    vec![
        ok_line(&format!("Already downloaded {}", abbrev_home(path))),
        detail("use --force to download again"),
    ]
}

/// Renders tool setup events. Quiet unless something is downloaded or goes wrong
/// (`verbose` also shows tools that were found, used by `yd update`).
pub struct ToolPrinter {
    title: &'static str,
    verbose: bool,
    live: Live,
    header: bool,
}

impl ToolPrinter {
    pub fn new(verbose: bool) -> Self {
        Self::with_title("Setting up tools", verbose)
    }

    pub fn with_title(title: &'static str, verbose: bool) -> Self {
        ToolPrinter {
            title,
            verbose,
            live: Live::new(),
            header: false,
        }
    }

    fn header(&mut self) {
        if !self.header {
            println!("{}", heading(self.title));
            self.header = true;
        }
    }

    pub fn event(&mut self, ev: ToolEvent) {
        match ev {
            ToolEvent::Found { name, path, source } => {
                if self.verbose {
                    self.header();
                    println!("{}", detail(&format!("{name}  {} ({source})", abbrev_home(&path))));
                }
            }
            ToolEvent::Downloading { name, done, total } => {
                self.header();
                let pct = total.filter(|t| *t > 0).map(|t| done as f64 / t as f64 * 100.0);
                let body = match pct {
                    Some(p) => format!("{:>3}% {}  {}", p as u32, progress_bar(p, BAR_WIDTH), dim(name)),
                    None => format!("{}  {}", dim(fmt_bytes(done)), dim(name)),
                };
                let line = format!("  {} {}", spinner_frame(self.live.tick), body);
                self.live.draw(&line, false);
            }
            ToolEvent::Installed { name, path } => {
                self.live.clear();
                self.header();
                println!("{}", detail(&format!("{name}  {}", abbrev_home(&path))));
                flush();
            }
            ToolEvent::Warn(msg) => {
                self.live.clear();
                println!("{}", warn_line(&msg));
                flush();
            }
        }
    }

    pub fn finish(&mut self) {
        self.live.clear();
    }
}
