//! External tool provisioning: yt-dlp, ffmpeg/ffprobe, deno.
//! Lookup order: tools dir -> PATH (yt-dlp only if recent enough) -> auto download.
//! Public interface (`Tools`, `ToolOptions`, `ToolEvent`, `tools_dir`, `ensure_tools`, `update_tools`) is FIXED.
//! Nothing here prints; all status goes through `ToolEvent` callbacks.
//!
//! Layout: `platform` (OS/arch asset matrix), `download` (HTTP, extraction, atomic install).

pub(crate) mod download;
pub(crate) mod platform;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Result, anyhow};

use download::{TempGuard, temp_path};
use platform::{
    BUNDLE_DIR, RemoteArchive, Target, YTDLP_BASE, YtdlpAsset, make_executable, which_in,
};

/// A PATH yt-dlp older than this is ignored (distro packages are often years stale and break on YouTube changes).
const MAX_YTDLP_AGE_DAYS: i64 = 90;
const VERSION_TIMEOUT: Duration = Duration::from_secs(15);

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
    Found {
        name: &'static str,
        path: PathBuf,
        source: &'static str,
    },
    /// Download started / progressing. `total` may be unknown.
    Downloading {
        name: &'static str,
        done: u64,
        total: Option<u64>,
    },
    /// Download + extraction finished.
    Installed { name: &'static str, path: PathBuf },
    /// Non-fatal problem (e.g. PATH yt-dlp too old, ffmpeg unavailable on this platform).
    Warn(String),
}

/// Directory where downloaded tools live: `<data_dir>/yd/tools` (e.g. ~/.local/share/yd/tools).
/// The environment variable `YD_TOOLS_DIR` (non-empty) overrides it; used by tests and portable setups.
pub fn tools_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("YD_TOOLS_DIR").filter(|d| !d.is_empty()) {
        return PathBuf::from(dir);
    }
    dirs::data_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("yd")
        .join("tools")
}

/// Ensure tools exist. yt-dlp missing => Err. ffmpeg/deno failures => Warn + None.
/// Lookup order: tools dir -> PATH -> download.
pub fn ensure_tools(opts: &ToolOptions, report: &mut dyn FnMut(ToolEvent)) -> Result<Tools> {
    provision(&Ctx::from_env(), opts, false, report)
}

/// `yd update`: force-refresh yt-dlp into the tools dir (even if the one in use is on PATH), and refresh
/// ffmpeg/deno only when they live in the tools dir. Tools not present anywhere are provisioned as in
/// `ensure_tools`; ones found on PATH are left alone.
pub fn update_tools(opts: &ToolOptions, report: &mut dyn FnMut(ToolEvent)) -> Result<Tools> {
    provision(&Ctx::from_env(), opts, true, report)
}

struct Ctx {
    dir: PathBuf,
    target: Target,
    path_var: Option<OsString>,
}

impl Ctx {
    fn from_env() -> Self {
        Self {
            dir: tools_dir(),
            target: Target::current(),
            path_var: std::env::var_os("PATH"),
        }
    }

    fn which(&self, name: &str) -> Option<PathBuf> {
        which_in(
            name,
            self.path_var.as_deref(),
            self.target.os == platform::Os::Windows,
        )
    }
}

fn provision(
    ctx: &Ctx,
    opts: &ToolOptions,
    update: bool,
    report: &mut dyn FnMut(ToolEvent),
) -> Result<Tools> {
    let ytdlp = resolve_ytdlp(ctx, update, report)?;
    let ffmpeg_dir = resolve_ffmpeg(ctx, update, report);
    let deno_dir = if opts.skip_deno {
        None
    } else {
        resolve_deno(ctx, update, report)
    };
    Ok(Tools {
        ytdlp,
        ffmpeg_dir,
        deno_dir,
    })
}

// ---------------------------------------------------------------- yt-dlp

fn local_ytdlp_path(ctx: &Ctx, asset: Option<&YtdlpAsset>) -> PathBuf {
    match asset.and_then(|a| a.bundle_exe) {
        Some(exe) => ctx.dir.join(BUNDLE_DIR).join(exe),
        None => ctx.dir.join(ctx.target.exe_name("yt-dlp")),
    }
}

fn resolve_ytdlp(ctx: &Ctx, force: bool, report: &mut dyn FnMut(ToolEvent)) -> Result<PathBuf> {
    let asset = ctx.target.ytdlp_asset();
    let local = local_ytdlp_path(ctx, asset.as_ref());

    if !force {
        if local.is_file() {
            let _ = make_executable(&local);
            report(ToolEvent::Found {
                name: "yt-dlp",
                path: local.clone(),
                source: "tools",
            });
            return Ok(local);
        }
        if let Some(on_path) = ctx.which("yt-dlp") {
            match read_ytdlp_version(&on_path) {
                None => report(ToolEvent::Warn(format!(
                    "yt-dlp at {} did not report a version; not using it",
                    on_path.display()
                ))),
                Some(v) if v.age_days(today_days()) > MAX_YTDLP_AGE_DAYS => {
                    report(ToolEvent::Warn(format!(
                        "yt-dlp {} at {} is outdated; fetching a fresh copy",
                        v.text,
                        on_path.display()
                    )))
                }
                Some(_) => {
                    report(ToolEvent::Found {
                        name: "yt-dlp",
                        path: on_path.clone(),
                        source: "PATH",
                    });
                    return Ok(on_path);
                }
            }
        }
    }

    let asset =
        asset.ok_or_else(|| anyhow!("yt-dlp: unsupported platform ({})", ctx.target.describe()))?;
    std::fs::create_dir_all(&ctx.dir)?;
    download_ytdlp(ctx, &asset, &local, report)?;
    if !local.is_file() {
        return Err(anyhow!("yt-dlp: could not install to {}", local.display()));
    }
    let _ = make_executable(&local);
    report(ToolEvent::Installed {
        name: "yt-dlp",
        path: local.clone(),
    });
    Ok(local)
}

fn download_ytdlp(
    ctx: &Ctx,
    asset: &YtdlpAsset,
    local: &Path,
    report: &mut dyn FnMut(ToolEvent),
) -> Result<()> {
    let url = format!("{YTDLP_BASE}{}", asset.name);
    let mut progress = |done, total| {
        report(ToolEvent::Downloading {
            name: "yt-dlp",
            done,
            total,
        })
    };
    match asset.bundle_exe {
        None => {
            let tmp = TempGuard::new(temp_path(&ctx.dir, "yt-dlp", "download"));
            download::download_file(&url, tmp.path(), &mut progress)?;
            make_executable(tmp.path())?;
            download::install(tmp, local)
        }
        Some(exe) => {
            // onedir bundle: executable + `_internal/`, replaced as a whole directory.
            let zip = TempGuard::new(temp_path(&ctx.dir, BUNDLE_DIR, "download"));
            download::download_file(&url, zip.path(), &mut progress)?;
            let staging = TempGuard::new(temp_path(&ctx.dir, BUNDLE_DIR, "new"));
            download::extract_all_zip(zip.path(), staging.path())?;
            make_executable(&staging.path().join(exe))?;
            download::install_dir(staging, &ctx.dir.join(BUNDLE_DIR))
        }
    }
}

// ---------------------------------------------------------------- ffmpeg / deno

fn resolve_ffmpeg(ctx: &Ctx, refresh: bool, report: &mut dyn FnMut(ToolEvent)) -> Option<PathBuf> {
    let local = ctx.dir.join(ctx.target.exe_name("ffmpeg"));
    let archives = ctx.target.ffmpeg_archives();
    let label = |a: &RemoteArchive| {
        if a.files.iter().any(|f| f.starts_with("ffprobe")) && a.files.len() == 1 {
            "ffprobe"
        } else {
            "ffmpeg"
        }
    };
    let hint = format!(
        "ffmpeg: no automatic download for {}; {}",
        ctx.target.describe(),
        ctx.target.manual_ffmpeg_hint()
    );
    resolve_archive_tool(
        ctx, "ffmpeg", &local, &archives, label, &hint, refresh, report,
    )
}

fn resolve_deno(ctx: &Ctx, refresh: bool, report: &mut dyn FnMut(ToolEvent)) -> Option<PathBuf> {
    let local = ctx.dir.join(ctx.target.exe_name("deno"));
    let archives: Vec<RemoteArchive> = ctx.target.deno_archive().into_iter().collect();
    let hint = format!(
        "deno: no automatic download for {}; some YouTube formats may be unavailable (https://github.com/yt-dlp/yt-dlp/wiki/EJS)",
        ctx.target.describe()
    );
    resolve_archive_tool(
        ctx,
        "deno",
        &local,
        &archives,
        |_| "deno",
        &hint,
        refresh,
        report,
    )
}

/// Shared flow for tools that come out of archives. Returns the directory containing the tool.
/// Never errors: problems become `Warn` + None (or the existing install on a failed refresh).
#[allow(clippy::too_many_arguments)]
fn resolve_archive_tool(
    ctx: &Ctx,
    name: &'static str,
    local: &Path,
    archives: &[RemoteArchive],
    label: impl Fn(&RemoteArchive) -> &'static str,
    unsupported_hint: &str,
    refresh: bool,
    report: &mut dyn FnMut(ToolEvent),
) -> Option<PathBuf> {
    let have_local = local.is_file();
    if have_local && !refresh {
        found_local(ctx, name, local, report);
        return Some(ctx.dir.clone());
    }
    if !have_local && let Some(on_path) = ctx.which(name) {
        report(ToolEvent::Found {
            name,
            path: on_path.clone(),
            source: "PATH",
        });
        return on_path.parent().map(Path::to_path_buf);
    }
    if archives.is_empty() {
        if have_local {
            found_local(ctx, name, local, report);
            return Some(ctx.dir.clone());
        }
        report(ToolEvent::Warn(unsupported_hint.to_string()));
        return None;
    }

    let result = std::fs::create_dir_all(&ctx.dir)
        .map_err(anyhow::Error::from)
        .and_then(|()| {
            for a in archives {
                download_archive(ctx, a, label(a), report)?;
            }
            Ok(())
        });
    if let Err(e) = &result {
        report(ToolEvent::Warn(format!("{name}: download failed: {e:#}")));
    }
    if local.is_file() {
        let _ = make_executable(local);
        if result.is_ok() {
            report(ToolEvent::Installed {
                name,
                path: local.to_path_buf(),
            });
        }
        Some(ctx.dir.clone())
    } else {
        report(ToolEvent::Warn(format!("{name}: could not be installed")));
        None
    }
}

fn found_local(ctx: &Ctx, name: &'static str, local: &Path, report: &mut dyn FnMut(ToolEvent)) {
    // A hand-placed binary may lack the exec bit; ffprobe sits next to ffmpeg.
    let _ = make_executable(local);
    if name == "ffmpeg" {
        let probe = ctx.dir.join(ctx.target.exe_name("ffprobe"));
        if probe.is_file() {
            let _ = make_executable(&probe);
        }
    }
    report(ToolEvent::Found {
        name,
        path: local.to_path_buf(),
        source: "tools",
    });
}

fn download_archive(
    ctx: &Ctx,
    a: &RemoteArchive,
    label: &'static str,
    report: &mut dyn FnMut(ToolEvent),
) -> Result<()> {
    let tmp = TempGuard::new(temp_path(&ctx.dir, label, "download"));
    download::download_file(&a.url, tmp.path(), &mut |done, total| {
        report(ToolEvent::Downloading {
            name: label,
            done,
            total,
        })
    })?;
    let got = download::extract_files(tmp.path(), a.kind, &a.files, &ctx.dir)?;
    let missing: Vec<&str> = a
        .files
        .iter()
        .filter(|f| !got.contains(f))
        .map(String::as_str)
        .collect();
    if !missing.is_empty() {
        report(ToolEvent::Warn(format!(
            "{label}: archive did not contain {}",
            missing.join(", ")
        )));
    }
    Ok(())
}

// ---------------------------------------------------------------- version handling

#[derive(Debug, PartialEq, Eq)]
struct YtdlpVersion {
    text: String,
    /// Release date as days since 1970-01-01.
    released_days: i64,
}

impl YtdlpVersion {
    fn age_days(&self, today: i64) -> i64 {
        today - self.released_days
    }
}

/// Parse `2026.08.19` or `2026.08.19.123456` (dev builds append a suffix; only the first 3 parts count).
fn parse_ytdlp_version(output: &str) -> Option<YtdlpVersion> {
    let text = output.trim();
    let mut parts = text.split('.');
    let y: i64 = parts.next()?.parse().ok()?;
    let m: i64 = parts.next()?.parse().ok()?;
    let d: i64 = parts.next()?.parse().ok()?;
    if !(2000..=9999).contains(&y) || !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    Some(YtdlpVersion {
        text: text.to_string(),
        released_days: days_from_civil(y, m, d),
    })
}

/// Days since 1970-01-01 for a proleptic Gregorian date (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn today_days() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| (d.as_secs() / 86_400) as i64)
}

/// Run `<path> --version` with a timeout; None if it fails, hangs, or prints something unparseable.
fn read_ytdlp_version(path: &Path) -> Option<YtdlpVersion> {
    let mut child = Command::new(path)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = std::time::Instant::now() + VERSION_TIMEOUT;
    let status = loop {
        match child.try_wait().ok()? {
            Some(s) => break s,
            None if std::time::Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            None => std::thread::sleep(Duration::from_millis(20)),
        }
    };
    if !status.success() {
        return None;
    }
    let mut out = String::new();
    std::io::Read::read_to_string(&mut child.stdout.take()?, &mut out).ok()?;
    parse_ytdlp_version(&out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_release_and_dev_versions() {
        let v = parse_ytdlp_version("2026.08.19\n").unwrap();
        assert_eq!(v.text, "2026.08.19");
        assert_eq!(v.released_days, days_from_civil(2026, 8, 19));
        let dev = parse_ytdlp_version("2026.08.19.123456").unwrap();
        assert_eq!(dev.released_days, v.released_days);
        assert!(parse_ytdlp_version("").is_none());
        assert!(parse_ytdlp_version("yt-dlp 2026.08.19").is_none());
        assert!(parse_ytdlp_version("2026.13.01").is_none());
        assert!(parse_ytdlp_version("2026.08").is_none());
    }

    #[test]
    fn civil_days_known_values() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 3, 1), 11_017);
        assert_eq!(
            days_from_civil(2024, 2, 29) + 1,
            days_from_civil(2024, 3, 1)
        );
    }

    #[test]
    fn age_is_measured_in_days() {
        let v = parse_ytdlp_version("2026.01.01").unwrap();
        let t = days_from_civil(2026, 4, 2); // 91 days later
        assert_eq!(v.age_days(t), 91);
        assert!(v.age_days(t) > MAX_YTDLP_AGE_DAYS);
        assert!(v.age_days(days_from_civil(2026, 4, 1)) <= MAX_YTDLP_AGE_DAYS);
    }

    #[test]
    fn which_in_finds_only_executables() {
        let dir = std::env::temp_dir().join(format!("yd-which-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("sometool");
        std::fs::write(&f, b"x").unwrap();
        let path = std::env::join_paths([&dir]).unwrap();
        #[cfg(unix)]
        assert!(which_in("sometool", Some(&path), false).is_none()); // no exec bit
        make_executable(&f).unwrap();
        assert_eq!(which_in("sometool", Some(&path), false), Some(f));
        assert!(which_in("missing", Some(&path), false).is_none());
        assert!(which_in("sometool", None, false).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Real provisioning into a temp dir with an empty PATH (everything must be downloaded; ~150MB).
    /// Run: `cargo test --release provision_real -- --ignored --nocapture`
    /// Dir defaults to /tmp/claude-1000/yd-tools-test; override with YD_TOOLS_TEST_DIR.
    #[test]
    #[ignore = "downloads ~150MB from the network"]
    fn provision_real() {
        let dir = std::env::var("YD_TOOLS_TEST_DIR")
            .unwrap_or_else(|_| "/tmp/claude-1000/yd-tools-test".into());
        let _ = std::fs::remove_dir_all(&dir);
        // SAFETY: single ignored test; no other thread reads the environment concurrently.
        unsafe {
            std::env::set_var("YD_TOOLS_DIR", &dir);
            std::env::set_var("PATH", "");
        }
        assert_eq!(tools_dir(), PathBuf::from(&dir));

        let mut events = Vec::new();
        let mut last_print = std::time::Instant::now();
        let tools = ensure_tools(&ToolOptions::default(), &mut |e| {
            if let ToolEvent::Downloading { .. } = e {
                if last_print.elapsed() < Duration::from_secs(2) {
                    return;
                }
                last_print = std::time::Instant::now();
            }
            eprintln!("{e:?}");
            events.push(e);
        })
        .unwrap();

        let ffdir = tools.ffmpeg_dir.expect("ffmpeg");
        let denodir = tools.deno_dir.expect("deno");
        let run = |p: PathBuf, arg: &str| {
            let out = Command::new(&p)
                .arg(arg)
                .output()
                .unwrap_or_else(|e| panic!("{}: {e}", p.display()));
            assert!(out.status.success(), "{} failed", p.display());
            eprintln!(
                "{}: {}",
                p.display(),
                String::from_utf8_lossy(&out.stdout)
                    .lines()
                    .next()
                    .unwrap_or("")
            );
        };
        run(tools.ytdlp.clone(), "--version");
        run(ffdir.join("ffmpeg"), "-version");
        run(ffdir.join("ffprobe"), "-version");
        run(denodir.join("deno"), "--version");

        // Second call must find everything in the tools dir without downloading.
        let mut again = Vec::new();
        ensure_tools(&ToolOptions::default(), &mut |e| again.push(e)).unwrap();
        assert!(
            again.iter().all(|e| matches!(
                e,
                ToolEvent::Found {
                    source: "tools",
                    ..
                }
            )),
            "{again:?}"
        );
        // update: everything lives in our tools dir, so all three are re-downloaded and re-installed.
        let mut upd = Vec::new();
        update_tools(&ToolOptions::default(), &mut |e| upd.push(e)).unwrap();
        for n in ["yt-dlp", "ffmpeg", "deno"] {
            assert!(
                upd.iter()
                    .any(|e| matches!(e, ToolEvent::Installed { name, .. } if *name == n)),
                "{n}: {upd:?}"
            );
        }
        run(tools.ytdlp.clone(), "--version");
        // No leftover temp files.
        let junk: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter(|e| {
                let n = e.file_name().to_string_lossy().into_owned();
                n.ends_with(".download") || n.ends_with(".tmp") || n.ends_with(".new")
            })
            .collect();
        assert!(junk.is_empty(), "{junk:?}");
    }
}
