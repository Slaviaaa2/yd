//! System clipboard text, read only when asked (Ctrl+V). `arboard` first, then external tools.

use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_millis(1200);

/// Clipboard text, or `None` when no method works (or the clipboard is empty / not text).
pub fn read_text() -> Option<String> {
    arboard_text()
        .or_else(external_text)
        .map(|t| t.replace("\r\n", "\n"))
        .filter(|t| !t.trim().is_empty())
}

fn arboard_text() -> Option<String> {
    let (tx, rx) = mpsc::channel();
    // A thread, so a stuck compositor connection cannot freeze the UI.
    std::thread::spawn(move || {
        let _ = tx.send(arboard::Clipboard::new().and_then(|mut c| c.get_text()).ok());
    });
    rx.recv_timeout(TIMEOUT).ok().flatten()
}

/// Commands per platform, tried in order.
fn external_commands() -> Vec<(&'static str, Vec<&'static str>)> {
    if cfg!(target_os = "macos") {
        vec![("pbpaste", vec![])]
    } else if cfg!(target_os = "windows") {
        vec![("powershell", vec!["-NoProfile", "-Command", "Get-Clipboard"])]
    } else {
        vec![
            ("wl-paste", vec!["--no-newline"]),
            ("xclip", vec!["-selection", "clipboard", "-o"]),
            ("xsel", vec!["-b"]),
        ]
    }
}

fn external_text() -> Option<String> {
    external_commands().into_iter().find_map(|(prog, args)| run_with_timeout(prog, &args))
}

fn run_with_timeout(prog: &str, args: &[&str]) -> Option<String> {
    let mut child = Command::new(prog)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut s = String::new();
        let _ = tx.send(stdout.read_to_string(&mut s).ok().map(|_| s));
    });
    match rx.recv_timeout(TIMEOUT) {
        Ok(Some(text)) => {
            let ok = child.wait().is_ok_and(|s| s.success());
            ok.then_some(text)
        }
        _ => {
            let _ = child.kill();
            let _ = child.wait();
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_tool_is_none_and_commands_exist() {
        assert_eq!(run_with_timeout("yd-no-such-clipboard-tool", &[]), None);
        assert!(!external_commands().is_empty());
        assert_eq!(run_with_timeout("sh", &["-c", "printf hi"]).as_deref(), Some("hi"));
        assert_eq!(run_with_timeout("sh", &["-c", "exit 1"]), None);
    }
}
