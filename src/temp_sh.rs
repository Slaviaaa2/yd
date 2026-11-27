//! Stage downloads separately so existing local files are never removed by an upload.

use std::collections::HashSet;
use std::fs::{self, DirBuilder, File};
use std::io::{self, Cursor, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail, ensure};

use crate::format::BuiltArgs;
use crate::tools::Tools;
use crate::ytdlp::{self, Event, Job, Outcome};

const UPLOAD_URL: &str = "https://temp.sh/upload";

pub fn run(
    job: &Job,
    built: &BuiltArgs,
    tools: &Tools,
    cancel: &AtomicBool,
    mut event: impl FnMut(Event),
) -> Result<Outcome> {
    let dest = staging_dir()?;
    let mut job = job.clone();
    job.dest = dest.clone();
    let downloaded = ytdlp::run_download(&job, built, tools, cancel, |ev| {
        if !matches!(ev, Event::Done { .. }) {
            event(ev);
        }
    });
    let result = downloaded.map(|mut outcome| {
        if outcome.success() {
            upload_saved(&dest, &mut outcome, cancel, &mut event);
        }
        outcome
    });
    // Only remove an empty staging folder. Partial downloads and auxiliary files remain recoverable.
    if fs::remove_dir(&dest).is_err() {
        event(Event::UploadRetained(dest));
    }
    result
}

fn staging_dir() -> Result<PathBuf> {
    let root = std::env::temp_dir().canonicalize()?;
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let dest = root.join(format!("yd-upload-{}-{stamp}", std::process::id()));
    let mut builder = DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
        .create(&dest)
        .with_context(|| format!("cannot create {}", dest.display()))?;
    Ok(dest)
}

fn upload_saved(
    dest: &Path,
    outcome: &mut Outcome,
    cancel: &AtomicBool,
    event: &mut impl FnMut(Event),
) {
    if outcome.saved.is_empty() && !cancel.load(Ordering::SeqCst) {
        outcome.errors += 1;
        event(Event::Error("No new files to upload to temp.sh".into()));
        return;
    }

    let agent: ureq::Agent = ureq::Agent::config_builder()
        .max_redirects(0)
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_per_read(Some(Duration::from_secs(30)))
        .timeout_per_write(Some(Duration::from_secs(30)))
        .build()
        .into();
    let mut seen = HashSet::new();
    for path in &outcome.saved {
        if cancel.load(Ordering::SeqCst) {
            break;
        }
        if !seen.insert(path) {
            continue;
        }
        event(Event::Uploading);
        let result = upload_one(dest, path, UPLOAD_URL, &agent, cancel, event);
        if let Err(err) = result {
            if cancel.load(Ordering::SeqCst) {
                break;
            }
            outcome.errors += 1;
            event(Event::Error(format!("{}: {err:#}", path.display())));
        }
    }
    outcome.cancelled = cancel.load(Ordering::SeqCst);
}

fn upload_one(
    dest: &Path,
    path: &Path,
    endpoint: &str,
    agent: &ureq::Agent,
    cancel: &AtomicBool,
    event: &mut impl FnMut(Event),
) -> Result<()> {
    let canonical = path
        .canonicalize()
        .context("cannot locate downloaded file")?;
    ensure!(
        canonical.starts_with(dest) && path.symlink_metadata()?.is_file(),
        "upload requires a regular file in the temporary download folder"
    );

    let link = upload_file(&canonical, endpoint, agent, cancel)?;
    event(Event::Uploaded(link));
    if cancel.load(Ordering::SeqCst) {
        bail!("interrupted");
    }
    fs::remove_file(&canonical).context("uploaded, but could not delete the local file")?;
    for parent in canonical.ancestors().skip(1).take_while(|p| *p != dest) {
        if fs::remove_dir(parent).is_err() {
            break;
        }
    }
    Ok(())
}

fn upload_file(
    path: &Path,
    endpoint: &str,
    agent: &ureq::Agent,
    cancel: &AtomicBool,
) -> Result<String> {
    let file = File::open(path)?;
    let size = file.metadata()?.len();
    ensure!(size <= 4_000_000_000, "temp.sh accepts files up to 4 GB");
    let filename = path
        .file_name()
        .context("missing filename")?
        .to_string_lossy();
    let filename = filename.replace(['\r', '\n', '"', '\\'], "_");
    let boundary = format!(
        "yd-{}-{:x}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
    );
    let header = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\n\
         Content-Type: application/octet-stream\r\n\r\n"
    );
    let trailer = format!("\r\n--{boundary}--\r\n");
    let len = header.len() as u64 + size + trailer.len() as u64;
    let mut body = CancelRead {
        inner: Cursor::new(header).chain(file).chain(Cursor::new(trailer)),
        cancel,
    };
    let mut response = agent
        .post(endpoint)
        .header(
            "Content-Type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .header("Content-Length", len.to_string())
        .send(ureq::SendBody::from_reader(&mut body))
        .context("temp.sh upload failed")?;
    ensure!(
        response.status().is_success(),
        "temp.sh returned HTTP {}",
        response.status()
    );
    let reply = response
        .body_mut()
        .with_config()
        .limit(8192)
        .read_to_string()?;
    let link = reply.trim();
    ensure!(
        link.strip_prefix("https://temp.sh/")
            .is_some_and(|path| !path.is_empty())
            && !link.chars().any(|c| c.is_whitespace() || c.is_control()),
        "temp.sh did not return a download link"
    );
    Ok(link.into())
}

struct CancelRead<'a, R> {
    inner: R,
    cancel: &'a AtomicBool,
}

impl<R: Read> Read for CancelRead<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.cancel.load(Ordering::SeqCst) {
            return Err(io::Error::other("interrupted"));
        }
        self.inner.read(buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;

    fn serve(reply: &'static str) -> (String, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/upload", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = BufReader::new(&mut socket);
            let mut line = String::new();
            request.read_line(&mut line).unwrap();
            assert!(line.starts_with("POST /upload "));
            let mut size = None;
            loop {
                line.clear();
                assert_ne!(request.read_line(&mut line).unwrap(), 0);
                if line == "\r\n" {
                    break;
                }
                if let Some(len) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    size = Some(len.trim().parse::<usize>().unwrap());
                }
            }
            let mut body = vec![0; size.unwrap()];
            request.read_exact(&mut body).unwrap();
            let body = std::str::from_utf8(&body).unwrap();
            assert!(body.contains("name=\"file\"; filename=\"sample.mp4\""));
            assert!(body.contains("\r\n\r\nsample data\r\n"));
            write!(
                socket,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                reply.len()
            )
            .unwrap();
        });
        (url, server)
    }

    #[test]
    fn removes_file_only_when_server_returns_a_link() {
        let dest = staging_dir().unwrap();
        let playlist = dest.join("playlist");
        fs::create_dir(&playlist).unwrap();
        let path = playlist.join("sample.mp4");
        fs::write(&path, "sample data").unwrap();
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .proxy(None)
            .timeout_global(Some(Duration::from_secs(5)))
            .build()
            .into();
        let cancel = AtomicBool::new(false);
        let mut links = Vec::new();
        let mut on_event = |event| {
            if let Event::Uploaded(link) = event {
                links.push(link);
            }
        };

        let (endpoint, server) = serve("Unexpected reply");
        assert!(upload_one(&dest, &path, &endpoint, &agent, &cancel, &mut on_event).is_err());
        server.join().unwrap();
        assert!(path.is_file());

        let link = "https://temp.sh/abc/sample.mp4";
        let (endpoint, server) = serve(link);
        upload_one(&dest, &path, &endpoint, &agent, &cancel, &mut on_event).unwrap();
        server.join().unwrap();
        assert_eq!(links, [link]);
        assert!(!path.exists());
        assert!(!playlist.exists());
        fs::remove_dir(dest).unwrap();
    }
}
