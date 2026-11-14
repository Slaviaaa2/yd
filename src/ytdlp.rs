//! Run yt-dlp as a child process and turn its output into UI-agnostic events.
//!
//! Output is machine readable via `--print` / `--progress-template` lines carrying a marker prefix.
//! `--no-quiet` is needed so that "has already been downloaded" shows up (`--print` implies quiet).

#![allow(dead_code)] // public API also used by the interactive mode

use std::io::{BufRead, BufReader, Read};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::Duration;

use anyhow::{Context, Result};
use serde_json::Value;

use crate::format::BuiltArgs;
use crate::tools::Tools;

const M_INFO: &str = "YDI ";
const M_PROGRESS: &str = "YDP ";
const M_POST: &str = "YDQ ";
const M_DONE: &str = "YDD ";

#[derive(Debug, Clone)]
pub struct Job {
    pub url: String,
    /// Destination directory (`-P`).
    pub dest: PathBuf,
    /// Output template (relative to `dest`).
    pub template: String,
    pub force: bool,
    /// Download the whole list for watch?v=..&list=.. URLs.
    pub playlist: bool,
    pub cookies_from_browser: Option<String>,
    /// Raw passthrough args.
    pub extra: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Info {
    pub id: Option<String>,
    pub title: String,
    pub uploader: Option<String>,
    pub duration: Option<f64>,
    pub playlist_title: Option<String>,
    pub playlist_index: Option<u32>,
    pub n_entries: Option<u32>,
    pub extractor: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Event {
    Info(Info),
    Progress {
        percent: f64,
        downloaded: u64,
        total: Option<u64>,
        speed: Option<f64>,
        eta: Option<u64>,
    },
    /// Post-processor name as reported by yt-dlp (e.g. `ExtractAudio`, `Merger`).
    PostProcess(String),
    Done {
        path: PathBuf,
        size: Option<u64>,
    },
    AlreadyDownloaded {
        path: PathBuf,
    },
    Warning(String),
    Error(String),
}

#[derive(Debug, Clone, Default)]
pub struct Outcome {
    /// Files written by this run (not including already-downloaded ones).
    pub saved: Vec<PathBuf>,
    pub already: Vec<PathBuf>,
    pub errors: usize,
    pub cancelled: bool,
    /// yt-dlp exit status was 0.
    pub exit_ok: bool,
}

impl Outcome {
    pub fn success(&self) -> bool {
        self.exit_ok && !self.cancelled && self.errors == 0
    }
}

/// `watch?v=X&list=Y` style URL (a video inside a playlist).
pub fn is_video_in_playlist(url: &str) -> bool {
    url.contains("list=") && (url.contains("v=") || url.contains("youtu.be/"))
}

/// Full yt-dlp argument list for a job (without the program name).
pub fn build_args(job: &Job, built: &BuiltArgs, tools: &Tools) -> Vec<String> {
    let mut a: Vec<String> = [
        "--newline",
        "--progress",
        "--no-quiet",
        "--no-simulate",
        "--progress-template",
        "download:YDP %(progress.{status,downloaded_bytes,total_bytes,total_bytes_estimate,speed,eta})j",
        "--progress-template",
        "postprocess:YDQ %(progress.{status,postprocessor})j",
        "--print",
        "before_dl:YDI %(.{id,title,uploader,channel,duration,playlist_title,playlist_index,n_entries,extractor_key})j",
        "--print",
        "after_move:YDD %(.{filepath,filesize})j",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    a.extend(built.args.iter().cloned());
    if let Some(dir) = &tools.ffmpeg_dir {
        a.push("--ffmpeg-location".into());
        a.push(dir.to_string_lossy().into_owned());
    }
    a.push("-P".into());
    a.push(job.dest.to_string_lossy().into_owned());
    a.push("-o".into());
    a.push(job.template.clone());
    if job.force {
        a.push("--force-overwrites".into());
    }
    if !job.playlist && is_video_in_playlist(&job.url) {
        a.push("--no-playlist".into());
    }
    if let Some(b) = &job.cookies_from_browser {
        a.push("--cookies-from-browser".into());
        a.push(b.clone());
    }
    a.extend(job.extra.iter().cloned());
    a.push("--".into());
    a.push(job.url.clone());
    a
}

fn child_path(tools: &Tools) -> Option<std::ffi::OsString> {
    let deno = tools.deno_dir.as_ref()?;
    let mut paths = vec![deno.clone()];
    if let Some(cur) = std::env::var_os("PATH") {
        paths.extend(std::env::split_paths(&cur));
    }
    std::env::join_paths(paths).ok()
}

enum Line {
    Out(String),
    Err(String),
}

fn pump<R: Read + Send + 'static>(r: R, tx: mpsc::Sender<Line>, wrap: fn(String) -> Line) {
    std::thread::spawn(move || {
        let mut rd = BufReader::new(r);
        let mut buf = Vec::new();
        loop {
            buf.clear();
            match rd.read_until(b'\n', &mut buf) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    let s = String::from_utf8_lossy(&buf).trim_end_matches(['\n', '\r']).to_string();
                    if tx.send(wrap(s)).is_err() {
                        break;
                    }
                }
            }
        }
    });
}

/// Run yt-dlp for one URL, reporting events as they happen.
/// Set `cancel` (e.g. from a Ctrl-C handler) to kill the child; `Outcome::cancelled` is then true.
pub fn run(
    job: &Job,
    built: &BuiltArgs,
    tools: &Tools,
    cancel: &AtomicBool,
    mut on_event: impl FnMut(Event),
) -> Result<Outcome> {
    let args = build_args(job, built, tools);
    let mut cmd = Command::new(&tools.ytdlp);
    cmd.args(&args)
        .env("PYTHONIOENCODING", "utf-8")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(p) = child_path(tools) {
        cmd.env("PATH", p);
    }
    let mut child = cmd
        .spawn()
        .with_context(|| format!("cannot start {}", tools.ytdlp.display()))?;
    let (tx, rx) = mpsc::channel();
    pump(child.stdout.take().unwrap(), tx.clone(), Line::Out);
    pump(child.stderr.take().unwrap(), tx, Line::Err);

    let mut st = State::default();
    let mut outcome = Outcome::default();
    loop {
        if cancel.load(Ordering::SeqCst) {
            let _ = child.kill();
            outcome.cancelled = true;
            break;
        }
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(Line::Out(l)) => st.stdout_line(&l, &mut outcome, &mut on_event),
            Ok(Line::Err(l)) => st.stderr_line(&l, &mut outcome, &mut on_event),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    let status = child.wait()?;
    // Drain lines that arrived before exit (only when not cancelled).
    if !outcome.cancelled {
        while let Ok(l) = rx.try_recv() {
            match l {
                Line::Out(l) => st.stdout_line(&l, &mut outcome, &mut on_event),
                Line::Err(l) => st.stderr_line(&l, &mut outcome, &mut on_event),
            }
        }
    }
    outcome.exit_ok = status.success();
    if !outcome.exit_ok && !outcome.cancelled && outcome.errors == 0 {
        outcome.errors = 1;
        on_event(Event::Error(format!("yt-dlp exited with {status}")));
    }
    Ok(outcome)
}

#[derive(Default)]
struct State {
    /// Current entry already existed ("has already been downloaded" / "exists, skipping").
    already: bool,
}

impl State {
    fn stdout_line(&mut self, line: &str, out: &mut Outcome, on: &mut impl FnMut(Event)) {
        if let Some(j) = line.strip_prefix(M_INFO) {
            self.already = false;
            if let Some(info) = parse_info(j) {
                on(Event::Info(info));
            }
        } else if let Some(j) = line.strip_prefix(M_PROGRESS) {
            if let Some(ev) = parse_progress(j) {
                on(ev);
            }
        } else if let Some(j) = line.strip_prefix(M_POST) {
            if let Ok(v) = serde_json::from_str::<Value>(j)
                && v["status"] == "started"
                && let Some(name) = v["postprocessor"].as_str()
            {
                on(Event::PostProcess(name.to_string()));
            }
        } else if let Some(j) = line.strip_prefix(M_DONE) {
            if let Ok(v) = serde_json::from_str::<Value>(j)
                && let Some(p) = v["filepath"].as_str()
            {
                let path = PathBuf::from(p);
                if self.already {
                    out.already.push(path.clone());
                    on(Event::AlreadyDownloaded { path });
                } else {
                    let size = std::fs::metadata(&path)
                        .map(|m| m.len())
                        .ok()
                        .or_else(|| v["filesize"].as_u64());
                    out.saved.push(path.clone());
                    on(Event::Done { path, size });
                }
            }
        } else if (line.starts_with("[download] ") && line.ends_with(" has already been downloaded"))
            || (line.starts_with("[ExtractAudio] ") && line.contains("exists, skipping"))
            || (line.starts_with("[VideoConvertor] ") && line.contains("exists, skipping"))
        {
            self.already = true;
        }
    }

    fn stderr_line(&mut self, line: &str, out: &mut Outcome, on: &mut impl FnMut(Event)) {
        if let Some(m) = line.strip_prefix("ERROR:") {
            out.errors += 1;
            on(Event::Error(m.trim().to_string()));
        } else if let Some(m) = line.strip_prefix("WARNING:") {
            on(Event::Warning(m.trim().to_string()));
        }
    }
}

fn parse_info(j: &str) -> Option<Info> {
    let v: Value = serde_json::from_str(j).ok()?;
    let s = |k: &str| v[k].as_str().filter(|s| !s.is_empty()).map(String::from);
    Some(Info {
        id: s("id"),
        title: s("title").unwrap_or_else(|| "Untitled".into()),
        uploader: s("uploader").or_else(|| s("channel")),
        duration: v["duration"].as_f64(),
        playlist_title: s("playlist_title"),
        playlist_index: v["playlist_index"].as_u64().map(|n| n as u32),
        n_entries: v["n_entries"].as_u64().map(|n| n as u32),
        extractor: s("extractor_key"),
    })
}

fn parse_progress(j: &str) -> Option<Event> {
    let v: Value = serde_json::from_str(j).ok()?;
    if v["status"] != "downloading" {
        return None;
    }
    let downloaded = v["downloaded_bytes"].as_f64()? as u64;
    let total = v["total_bytes"]
        .as_f64()
        .or_else(|| v["total_bytes_estimate"].as_f64())
        .map(|t| t as u64);
    let percent = match total {
        Some(t) if t > 0 => (downloaded as f64 / t as f64 * 100.0).min(100.0),
        _ => 0.0,
    };
    Some(Event::Progress {
        percent,
        downloaded,
        total,
        speed: v["speed"].as_f64(),
        eta: v["eta"].as_f64().map(|e| e as u64),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playlist_url_detection() {
        assert!(is_video_in_playlist("https://www.youtube.com/watch?v=abc&list=PL1"));
        assert!(is_video_in_playlist("https://youtu.be/abc?list=PL1"));
        assert!(!is_video_in_playlist("https://www.youtube.com/playlist?list=PL1"));
        assert!(!is_video_in_playlist("https://www.youtube.com/watch?v=abc"));
    }

    #[test]
    fn parses_events() {
        let mut st = State::default();
        let mut out = Outcome::default();
        let mut evs = Vec::new();
        let mut on = |e: Event| evs.push(e);
        st.stdout_line(
            r#"YDI {"id":"x","title":"T","uploader":"U","duration":19,"playlist_index":2,"n_entries":5}"#,
            &mut out,
            &mut on,
        );
        st.stdout_line(
            r#"YDP {"status":"downloading","downloaded_bytes":50,"total_bytes":200,"speed":10.5,"eta":3}"#,
            &mut out,
            &mut on,
        );
        st.stdout_line("[download] a.mp4 has already been downloaded", &mut out, &mut on);
        st.stdout_line(r#"YDD {"filepath":"/tmp/a.mp4"}"#, &mut out, &mut on);
        st.stderr_line("ERROR: [youtube] x: boom", &mut out, &mut on);
        assert!(matches!(&evs[0], Event::Info(i) if i.title == "T" && i.playlist_index == Some(2)));
        assert!(matches!(&evs[1], Event::Progress { percent, .. } if (*percent - 25.0).abs() < 1e-9));
        assert!(matches!(&evs[2], Event::AlreadyDownloaded { .. }));
        assert!(matches!(&evs[3], Event::Error(m) if m.contains("boom")));
        assert_eq!(out.errors, 1);
    }
}
