//! External tool provisioning: yt-dlp, ffmpeg/ffprobe, deno.
//! Lookup order: tools dir -> PATH (yt-dlp only if recent enough) -> auto download.
//! OWNER: tools builder. Public interface below is FIXED (other modules depend on it).

use std::path::PathBuf;

/// Resolved tool locations.
#[derive(Debug, Clone)]
pub struct Tools {
    /// yt-dlp executable.
    pub ytdlp: PathBuf,
    /// Directory containing ffmpeg (+ffprobe); pass to `--ffmpeg-location`. None = unavailable.
    pub ffmpeg_dir: Option<PathBuf>,
    /// Directory containing deno; caller prepends it to PATH for yt-dlp. None = unavailable/skipped.
    pub deno_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, Default)]
pub struct ToolOptions {
    /// Do not provision deno.
    pub skip_deno: bool,
}

/// Progress/status events so the UI can render them in its own style. Never print directly.
#[derive(Debug, Clone)]
pub enum ToolEvent {
    /// Using an existing tool (source: "tools" | "PATH").
    Found { name: &'static str, path: PathBuf, source: &'static str },
    /// Download started / progressing. `total` may be unknown.
    Downloading { name: &'static str, done: u64, total: Option<u64> },
    /// Download + extraction finished.
    Installed { name: &'static str, path: PathBuf },
    /// Non-fatal problem (e.g. PATH yt-dlp too old, ffmpeg unavailable on this platform).
    Warn(String),
}

/// Directory where downloaded tools live: `<data_dir>/yd/tools` (e.g. ~/.local/share/yd/tools).
pub fn tools_dir() -> PathBuf {
    dirs::data_dir().unwrap_or_else(std::env::temp_dir).join("yd").join("tools")
}

// INTERIM (replace in tools builder): PATH lookup only, no download.
fn which(name: &str) -> Option<PathBuf> {
    let exe = if cfg!(windows) { format!("{name}.exe") } else { name.to_string() };
    std::env::split_paths(&std::env::var_os("PATH")?).map(|d| d.join(&exe)).find(|p| p.is_file())
}

/// Ensure tools exist. yt-dlp missing => Err. ffmpeg/deno failures => Warn + None.
pub fn ensure_tools(opts: &ToolOptions, report: &mut dyn FnMut(ToolEvent)) -> anyhow::Result<Tools> {
    let ytdlp = which("yt-dlp").ok_or_else(|| anyhow::anyhow!("yt-dlp not found"))?;
    report(ToolEvent::Found { name: "yt-dlp", path: ytdlp.clone(), source: "PATH" });
    let ffmpeg_dir = which("ffmpeg").and_then(|p| p.parent().map(PathBuf::from));
    let deno_dir = if opts.skip_deno { None } else { which("deno").and_then(|p| p.parent().map(PathBuf::from)) };
    Ok(Tools { ytdlp, ffmpeg_dir, deno_dir })
}

/// `yd update`: re-download yt-dlp into the tools dir (or `yt-dlp -U` if it is ours), refresh others if present.
pub fn update_tools(opts: &ToolOptions, report: &mut dyn FnMut(ToolEvent)) -> anyhow::Result<Tools> {
    let _ = (opts, report);
    todo!()
}
