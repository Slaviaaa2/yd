//! On-disk transcript files, with one writer per interactive session.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::Path;

use time::OffsetDateTime;

use crate::ui::{ansi, heading};

pub fn open_session(dir: &Path) -> io::Result<File> {
    fs::create_dir_all(dir)?;
    let now = OffsetDateTime::now_utc();
    let date = now.date();
    let name = format!(
        "{:04}-{:02}-{:02}T{:02}-{:02}-{:02}.{:09}Z-{}.log",
        date.year(),
        u8::from(date.month()),
        date.day(),
        now.hour(),
        now.minute(),
        now.second(),
        now.nanosecond(),
        std::process::id()
    );
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(dir.join(name))
}

pub fn append(file: &mut File, styled: &str) -> io::Result<()> {
    let plain: String = ansi::to_line(styled)
        .spans
        .iter()
        .flat_map(|span| span.content.chars())
        .filter(|c| !c.is_control() || *c == '\n')
        .collect();
    writeln!(file, "{plain}")
}

pub fn read_sessions(dir: &Path) -> io::Result<Vec<String>> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "log") && entry.file_type()?.is_file() {
            paths.push(path);
        }
    }
    paths.sort_unstable();

    let mut lines = Vec::new();
    for path in paths {
        let bytes = fs::read(&path)?;
        if bytes.is_empty() {
            continue;
        }
        let name = path.file_stem().unwrap_or_default().to_string_lossy();
        let timestamp = name.get(..19).map_or(name.to_string(), |stamp| {
            let (date, clock) = stamp.split_at(10);
            format!("{date} {} UTC", clock[1..].replace('-', ":"))
        });
        lines.push(heading(&format!("Session {timestamp}")));
        lines.extend(
            String::from_utf8_lossy(&bytes)
                .lines()
                .map(|line| line.chars().filter(|c| !c.is_control()).collect()),
        );
        lines.push(String::new());
    }
    Ok(lines)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::download::{TempGuard, temp_path};

    #[test]
    fn reads_inputs_and_results_in_session_order() {
        let path = temp_path(&std::env::temp_dir(), "yd-session-log-test", "tmp");
        fs::create_dir(&path).unwrap();
        let dir = TempGuard::new(path);
        let mut first = open_session(dir.path()).unwrap();
        append(&mut first, "\x1b[2m› https://example.com/video\x1b[0m").unwrap();
        append(&mut first, "✔ Saved /downloads/video.mp4").unwrap();
        drop(first);

        let mut second = open_session(dir.path()).unwrap();
        append(&mut second, "› https://example.com/other /o ex:tempsh").unwrap();
        append(&mut second, "✔ Uploaded https://temp.sh/a/other.mp4").unwrap();
        let lines = read_sessions(dir.path()).unwrap();
        assert_eq!(
            lines
                .iter()
                .filter(|line| line.contains("Session "))
                .count(),
            2
        );
        assert_eq!(lines[1], "› https://example.com/video");
        assert_eq!(lines[2], "✔ Saved /downloads/video.mp4");
        assert_eq!(lines[6], "✔ Uploaded https://temp.sh/a/other.mp4");
    }
}
