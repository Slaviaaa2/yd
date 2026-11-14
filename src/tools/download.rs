//! HTTP download and archive extraction. Never prints; progress goes through callbacks.

use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};

use super::platform::{ArchiveKind, make_executable};

const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

/// Deletes the path (file or dir) on drop unless disarmed. Keeps cancelled/failed downloads from leaving junk.
pub struct TempGuard {
    path: PathBuf,
    armed: bool,
}

impl TempGuard {
    pub fn new(path: PathBuf) -> Self {
        Self { path, armed: true }
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for TempGuard {
    fn drop(&mut self) {
        if self.armed {
            if self.path.is_dir() {
                let _ = fs::remove_dir_all(&self.path);
            } else {
                let _ = fs::remove_file(&self.path);
            }
        }
    }
}

/// Unique-ish sibling temp name: `<dir>/<stem>.<pid>.<suffix>`.
pub fn temp_path(dir: &Path, stem: &str, suffix: &str) -> PathBuf {
    dir.join(format!("{stem}.{}.{suffix}", std::process::id()))
}

/// Download `url` to `dest`. `progress(done, total)` is throttled to ~100ms, plus first and last call.
pub fn download_file(
    url: &str,
    dest: &Path,
    progress: &mut dyn FnMut(u64, Option<u64>),
) -> Result<()> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .user_agent("yd/1.0")
        .timeout_connect(Some(Duration::from_secs(30)))
        .build()
        .into();
    let mut resp = agent
        .get(url)
        .call()
        .with_context(|| format!("request failed: {url}"))?;
    let total = resp.body().content_length();
    let mut reader = resp.body_mut().as_reader();
    let mut out =
        File::create(dest).with_context(|| format!("cannot create {}", dest.display()))?;

    let mut buf = vec![0u8; 64 * 1024];
    let mut done = 0u64;
    progress(0, total);
    let mut last = Instant::now();
    loop {
        let n = reader
            .read(&mut buf)
            .with_context(|| format!("download interrupted: {url}"))?;
        if n == 0 {
            break;
        }
        out.write_all(&buf[..n])?;
        done += n as u64;
        if last.elapsed() >= PROGRESS_INTERVAL {
            progress(done, total);
            last = Instant::now();
        }
    }
    out.flush()?;
    progress(done, total.or(Some(done)));
    Ok(())
}

/// Map an archive entry name to a path under `root`, or None if it would escape (zip slip),
/// is absolute, or is empty. Accepts both `/` and `\` separators; `.` components are dropped.
pub fn safe_join(root: &Path, entry_name: &str) -> Option<PathBuf> {
    let normalized = entry_name.replace('\\', "/");
    let mut rel = PathBuf::new();
    for comp in Path::new(&normalized).components() {
        match comp {
            Component::Normal(c) => rel.push(c),
            Component::CurDir => {}
            // `..`, root dir, drive prefixes
            _ => return None,
        }
    }
    if rel.as_os_str().is_empty() {
        return None;
    }
    Some(root.join(rel))
}

fn base_name(entry_name: &str) -> &str {
    entry_name.rsplit(['/', '\\']).next().unwrap_or(entry_name)
}

/// If `entry_name`'s file name is still wanted, write it via `copy` to a temp file, chmod, and atomically
/// rename it into `dest_dir`. Returns the wanted name that was satisfied.
fn take_entry(
    entry_name: &str,
    remaining: &mut Vec<String>,
    dest_dir: &Path,
    copy: &mut dyn FnMut(&mut File) -> io::Result<()>,
) -> Result<Option<String>> {
    let file_name = base_name(entry_name);
    let Some(idx) = remaining
        .iter()
        .position(|w| w.eq_ignore_ascii_case(file_name))
    else {
        return Ok(None);
    };
    let name = remaining.remove(idx);
    let tmp = TempGuard::new(temp_path(dest_dir, &name, "tmp"));
    let mut f = File::create(tmp.path())?;
    copy(&mut f)?;
    drop(f);
    make_executable(tmp.path())?;
    install(tmp, &dest_dir.join(&name))?;
    Ok(Some(name))
}

/// Extract only the entries whose file name matches one of `wanted` (case-insensitive) into `dest_dir`
/// (flat). Each file is written to a temp name, made executable, then atomically renamed into place.
/// Returns the names extracted.
pub fn extract_files(
    archive: &Path,
    kind: ArchiveKind,
    wanted: &[String],
    dest_dir: &Path,
) -> Result<Vec<String>> {
    fs::create_dir_all(dest_dir)?;
    let mut remaining: Vec<String> = wanted.to_vec();
    let mut extracted = Vec::new();

    match kind {
        ArchiveKind::Zip => {
            let mut zip = zip::ZipArchive::new(File::open(archive)?)?;
            for i in 0..zip.len() {
                let mut entry = zip.by_index(i)?;
                if entry.is_dir() {
                    continue;
                }
                let Ok(name) = entry.name().map(|n| n.into_owned()) else {
                    continue;
                };
                if let Some(n) = take_entry(&name, &mut remaining, dest_dir, &mut |f| {
                    io::copy(&mut entry, f).map(|_| ())
                })? {
                    extracted.push(n);
                }
                if remaining.is_empty() {
                    break;
                }
            }
        }
        ArchiveKind::TarXz => {
            let xz = xz2::read::XzDecoder::new(io::BufReader::new(File::open(archive)?));
            let mut tar = tar::Archive::new(xz);
            for entry in tar.entries()? {
                let mut entry = entry?;
                if !entry.header().entry_type().is_file() {
                    continue;
                }
                let name = entry.path()?.to_string_lossy().into_owned();
                if let Some(n) = take_entry(&name, &mut remaining, dest_dir, &mut |f| {
                    io::copy(&mut entry, f).map(|_| ())
                })? {
                    extracted.push(n);
                }
                if remaining.is_empty() {
                    break;
                }
            }
        }
    }
    Ok(extracted)
}

/// Extract every entry of a zip into `dest_dir`, skipping entries that escape it (zip slip).
/// Restores Unix permission bits when the archive carries them.
pub fn extract_all_zip(archive: &Path, dest_dir: &Path) -> Result<()> {
    fs::create_dir_all(dest_dir)?;
    let mut zip = zip::ZipArchive::new(File::open(archive)?)?;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        let Ok(name) = entry.name().map(|n| n.into_owned()) else {
            continue;
        };
        let Some(target) = safe_join(dest_dir, &name) else {
            continue;
        };
        if entry.is_dir() {
            fs::create_dir_all(&target)?;
            continue;
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut f = File::create(&target)?;
        io::copy(&mut entry, &mut f)?;
        drop(f);
        #[cfg(unix)]
        if let Some(mode) = entry.unix_mode() {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&target, fs::Permissions::from_mode(mode & 0o777))?;
        }
    }
    Ok(())
}

/// Move the guarded temp file over `dest` (atomic rename on the same filesystem) and disarm the guard.
pub fn install(mut tmp: TempGuard, dest: &Path) -> Result<()> {
    fs::rename(tmp.path(), dest).with_context(|| format!("cannot install {}", dest.display()))?;
    tmp.disarm();
    Ok(())
}

/// Replace directory `dest` with the guarded temp directory.
pub fn install_dir(mut tmp: TempGuard, dest: &Path) -> Result<()> {
    if dest.exists() {
        fs::remove_dir_all(dest).with_context(|| format!("cannot replace {}", dest.display()))?;
    }
    fs::rename(tmp.path(), dest).map_err(|e| anyhow!("cannot install {}: {e}", dest.display()))?;
    tmp.disarm();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_join_accepts_normal_paths() {
        let root = Path::new("/r");
        assert_eq!(
            safe_join(root, "a/b.txt"),
            Some(PathBuf::from("/r/a/b.txt"))
        );
        assert_eq!(
            safe_join(root, "./a\\b.txt"),
            Some(PathBuf::from("/r/a/b.txt"))
        );
        assert_eq!(safe_join(root, "dir/"), Some(PathBuf::from("/r/dir")));
    }

    #[test]
    fn safe_join_rejects_escapes() {
        let root = Path::new("/r");
        assert_eq!(safe_join(root, "../evil"), None);
        assert_eq!(safe_join(root, "a/../../evil"), None);
        assert_eq!(safe_join(root, "a\\..\\..\\evil"), None);
        assert_eq!(safe_join(root, "/etc/passwd"), None);
        assert_eq!(safe_join(root, ""), None);
        assert_eq!(safe_join(root, "."), None);
    }

    #[test]
    fn base_name_handles_both_separators() {
        assert_eq!(base_name("x/bin/ffmpeg"), "ffmpeg");
        assert_eq!(base_name("x\\bin\\ffmpeg.exe"), "ffmpeg.exe");
        assert_eq!(base_name("ffmpeg"), "ffmpeg");
    }

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("yd-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn zip_extract_all_skips_slip_entries_and_extract_files_is_flat() {
        use zip::write::SimpleFileOptions;
        let dir = scratch("zip");
        let zpath = dir.join("t.zip");
        {
            let mut w = zip::ZipWriter::new(File::create(&zpath).unwrap());
            let o = SimpleFileOptions::default();
            w.start_file("pkg/bin/tool", o).unwrap();
            w.write_all(b"hello").unwrap();
            w.start_file("pkg/_internal/lib.so", o).unwrap();
            w.write_all(b"lib").unwrap();
            w.start_file("../escaped.txt", o).unwrap();
            w.write_all(b"bad").unwrap();
            w.finish().unwrap();
        }
        let out = dir.join("out");
        extract_all_zip(&zpath, &out).unwrap();
        assert!(out.join("pkg/bin/tool").is_file());
        assert!(out.join("pkg/_internal/lib.so").is_file());
        assert!(!dir.join("escaped.txt").exists());

        let flat = dir.join("flat");
        let got = extract_files(
            &zpath,
            ArchiveKind::Zip,
            &["TOOL".to_string(), "nope".to_string()],
            &flat,
        )
        .unwrap();
        assert_eq!(got, ["TOOL"]);
        assert_eq!(fs::read(flat.join("TOOL")).unwrap(), b"hello");
        // no temp leftovers
        assert_eq!(fs::read_dir(&flat).unwrap().count(), 1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn tar_xz_extract_files() {
        let dir = scratch("tarxz");
        let tpath = dir.join("t.tar.xz");
        {
            let enc = xz2::write::XzEncoder::new(File::create(&tpath).unwrap(), 1);
            let mut b = tar::Builder::new(enc);
            for (name, data) in [
                ("p/bin/ffmpeg", &b"F"[..]),
                ("p/bin/ffprobe", b"P"),
                ("p/doc/readme", b"R"),
            ] {
                let mut h = tar::Header::new_gnu();
                h.set_size(data.len() as u64);
                h.set_mode(0o644);
                h.set_cksum();
                b.append_data(&mut h, name, data).unwrap();
            }
            b.into_inner().unwrap().finish().unwrap();
        }
        let out = dir.join("out");
        let want = ["ffmpeg".to_string(), "ffprobe".to_string()];
        let got = extract_files(&tpath, ArchiveKind::TarXz, &want, &out).unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(fs::read(out.join("ffprobe")).unwrap(), b"P");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(out.join("ffmpeg"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o111,
                0o111
            );
        }
        let _ = fs::remove_dir_all(&dir);
    }
}
