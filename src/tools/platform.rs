//! Platform detection and the OS/arch -> release asset matrix.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(clippy::enum_variant_names)]
pub enum Os {
    Windows,
    Linux,
    MacOs,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arch {
    X64,
    Arm64,
    Armv7,
    X86,
    Other,
}

/// The machine we run on. Constructed explicitly in tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Target {
    pub os: Os,
    pub arch: Arch,
    /// Linux with musl libc (Alpine etc.).
    pub musl: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveKind {
    Zip,
    TarXz,
}

/// A downloadable archive from which only `files` are extracted (flat, into the tools dir).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteArchive {
    pub url: String,
    pub kind: ArchiveKind,
    pub files: Vec<String>,
}

/// yt-dlp release asset. `bundle_exe` is set for onedir zip distributions
/// (executable + `_internal/`) that must be extracted as a whole.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct YtdlpAsset {
    pub name: &'static str,
    pub bundle_exe: Option<&'static str>,
}

pub const BTBN_BASE: &str = "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/";
pub const YTDLP_BASE: &str = "https://github.com/yt-dlp/yt-dlp/releases/latest/download/";
pub const DENO_BASE: &str = "https://github.com/denoland/deno/releases/latest/download/";
pub const BUNDLE_DIR: &str = "yt-dlp-bundle";

impl Target {
    pub fn current() -> Self {
        let os = match std::env::consts::OS {
            "windows" => Os::Windows,
            "linux" => Os::Linux,
            "macos" => Os::MacOs,
            _ => Os::Other,
        };
        let arch = match std::env::consts::ARCH {
            "x86_64" => Arch::X64,
            "aarch64" => Arch::Arm64,
            "arm" => Arch::Armv7,
            "x86" => Arch::X86,
            _ => Arch::Other,
        };
        Self {
            os,
            arch,
            musl: os == Os::Linux && detect_musl(),
        }
    }

    /// Short human description, e.g. "linux-x64", "linux(musl)-arm64".
    pub fn describe(&self) -> String {
        let os = match self.os {
            Os::Windows => "windows",
            Os::MacOs => "macos",
            Os::Linux if self.musl => "linux(musl)",
            Os::Linux => "linux",
            Os::Other => "unknown",
        };
        let arch = match self.arch {
            Arch::X64 => "x64",
            Arch::Arm64 => "arm64",
            Arch::Armv7 => "armv7",
            Arch::X86 => "x86",
            Arch::Other => "unknown",
        };
        format!("{os}-{arch}")
    }

    pub fn exe_name(&self, base: &str) -> String {
        if self.os == Os::Windows {
            format!("{base}.exe")
        } else {
            base.to_string()
        }
    }

    pub fn ytdlp_asset(&self) -> Option<YtdlpAsset> {
        let single = |name| {
            Some(YtdlpAsset {
                name,
                bundle_exe: None,
            })
        };
        match (self.os, self.arch, self.musl) {
            (Os::Windows, Arch::X64, _) => single("yt-dlp.exe"),
            (Os::Windows, Arch::Arm64, _) => single("yt-dlp_arm64.exe"),
            (Os::Windows, Arch::X86, _) => single("yt-dlp_x86.exe"),
            (Os::MacOs, _, _) => single("yt-dlp_macos"),
            (Os::Linux, Arch::X64, false) => single("yt-dlp_linux"),
            (Os::Linux, Arch::Arm64, false) => single("yt-dlp_linux_aarch64"),
            (Os::Linux, Arch::X64, true) => single("yt-dlp_musllinux"),
            (Os::Linux, Arch::Arm64, true) => single("yt-dlp_musllinux_aarch64"),
            // 32-bit ARM ships only as a zip onedir bundle.
            (Os::Linux, Arch::Armv7, false) => Some(YtdlpAsset {
                name: "yt-dlp_linux_armv7l.zip",
                bundle_exe: Some("yt-dlp_linux_armv7l"),
            }),
            _ => None,
        }
    }

    /// deno publishes no official musl / 32-bit builds.
    pub fn deno_asset(&self) -> Option<String> {
        if self.musl {
            return None;
        }
        let arch = match self.arch {
            Arch::X64 => "x86_64",
            Arch::Arm64 => "aarch64",
            _ => return None,
        };
        match self.os {
            Os::Windows => Some(format!("deno-{arch}-pc-windows-msvc.zip")),
            Os::MacOs => Some(format!("deno-{arch}-apple-darwin.zip")),
            Os::Linux => Some(format!("deno-{arch}-unknown-linux-gnu.zip")),
            Os::Other => None,
        }
    }

    pub fn deno_archive(&self) -> Option<RemoteArchive> {
        Some(RemoteArchive {
            url: format!("{DENO_BASE}{}", self.deno_asset()?),
            kind: ArchiveKind::Zip,
            files: vec![self.exe_name("deno")],
        })
    }

    /// Empty = no automatic ffmpeg download for this platform.
    pub fn ffmpeg_archives(&self) -> Vec<RemoteArchive> {
        let files = vec![self.exe_name("ffmpeg"), self.exe_name("ffprobe")];
        match self.os {
            Os::Windows => {
                let slug = match self.arch {
                    Arch::X64 => "win64",
                    Arch::Arm64 => "winarm64",
                    _ => return vec![],
                };
                vec![RemoteArchive {
                    url: format!("{BTBN_BASE}ffmpeg-master-latest-{slug}-gpl.zip"),
                    kind: ArchiveKind::Zip,
                    files,
                }]
            }
            // evermeet.cx ships ffmpeg and ffprobe as separate zips (x86_64 builds; Rosetta on Apple Silicon).
            Os::MacOs => ["ffmpeg", "ffprobe"]
                .into_iter()
                .map(|n| RemoteArchive {
                    url: format!("https://evermeet.cx/ffmpeg/getrelease/{n}/zip"),
                    kind: ArchiveKind::Zip,
                    files: vec![n.to_string()],
                })
                .collect(),
            // BtbN builds target glibc, so no musl.
            Os::Linux if !self.musl => {
                let slug = match self.arch {
                    Arch::X64 => "linux64",
                    Arch::Arm64 => "linuxarm64",
                    _ => return vec![],
                };
                vec![RemoteArchive {
                    url: format!("{BTBN_BASE}ffmpeg-master-latest-{slug}-gpl.tar.xz"),
                    kind: ArchiveKind::TarXz,
                    files,
                }]
            }
            _ => vec![],
        }
    }

    pub fn manual_ffmpeg_hint(&self) -> &'static str {
        match self.os {
            Os::MacOs => "install it manually: brew install ffmpeg",
            Os::Linux if self.musl => "install it manually: apk add ffmpeg",
            Os::Linux => "install it manually: sudo apt install ffmpeg / sudo dnf install ffmpeg",
            _ => "install ffmpeg manually and add it to PATH or the tools dir",
        }
    }
}

fn detect_musl() -> bool {
    // Alpine and friends have /lib/ld-musl-<arch>.so.1 as the loader.
    std::fs::read_dir("/lib").is_ok_and(|rd| {
        rd.flatten().any(|e| {
            let n = e.file_name();
            let n = n.to_string_lossy();
            n.starts_with("ld-musl-") && n.ends_with(".so.1")
        })
    })
}

/// Find an executable in the given PATH value.
pub fn which_in(name: &str, path_var: Option<&OsStr>, windows: bool) -> Option<PathBuf> {
    let exe = if windows {
        format!("{name}.exe")
    } else {
        name.to_string()
    };
    std::env::split_paths(path_var?)
        .map(|d| d.join(&exe))
        .find(|p| is_executable_file(p))
}

pub fn is_executable_file(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.is_file() && meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        meta.is_file()
    }
}

/// chmod 0755 on Unix; no-op elsewhere.
pub fn make_executable(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(os: Os, arch: Arch, musl: bool) -> Target {
        Target { os, arch, musl }
    }

    #[test]
    fn ytdlp_matrix() {
        let name = |t: Target| t.ytdlp_asset().map(|a| a.name);
        assert_eq!(name(t(Os::Windows, Arch::X64, false)), Some("yt-dlp.exe"));
        assert_eq!(
            name(t(Os::Windows, Arch::Arm64, false)),
            Some("yt-dlp_arm64.exe")
        );
        assert_eq!(
            name(t(Os::Windows, Arch::X86, false)),
            Some("yt-dlp_x86.exe")
        );
        assert_eq!(name(t(Os::MacOs, Arch::Arm64, false)), Some("yt-dlp_macos"));
        assert_eq!(name(t(Os::Linux, Arch::X64, false)), Some("yt-dlp_linux"));
        assert_eq!(
            name(t(Os::Linux, Arch::Arm64, false)),
            Some("yt-dlp_linux_aarch64")
        );
        assert_eq!(
            name(t(Os::Linux, Arch::X64, true)),
            Some("yt-dlp_musllinux")
        );
        assert_eq!(
            name(t(Os::Linux, Arch::Arm64, true)),
            Some("yt-dlp_musllinux_aarch64")
        );
        assert_eq!(name(t(Os::Linux, Arch::Armv7, true)), None);
        let arm = t(Os::Linux, Arch::Armv7, false).ytdlp_asset().unwrap();
        assert_eq!(arm.name, "yt-dlp_linux_armv7l.zip");
        assert_eq!(arm.bundle_exe, Some("yt-dlp_linux_armv7l"));
        assert_eq!(name(t(Os::Other, Arch::X64, false)), None);
    }

    #[test]
    fn deno_matrix() {
        let d = |t: Target| t.deno_asset();
        assert_eq!(
            d(t(Os::Windows, Arch::X64, false)).unwrap(),
            "deno-x86_64-pc-windows-msvc.zip"
        );
        assert_eq!(
            d(t(Os::MacOs, Arch::Arm64, false)).unwrap(),
            "deno-aarch64-apple-darwin.zip"
        );
        assert_eq!(
            d(t(Os::Linux, Arch::X64, false)).unwrap(),
            "deno-x86_64-unknown-linux-gnu.zip"
        );
        assert_eq!(
            d(t(Os::Linux, Arch::Arm64, false)).unwrap(),
            "deno-aarch64-unknown-linux-gnu.zip"
        );
        assert!(d(t(Os::Linux, Arch::X64, true)).is_none());
        assert!(d(t(Os::Linux, Arch::Armv7, false)).is_none());
    }

    #[test]
    fn ffmpeg_matrix() {
        let w = t(Os::Windows, Arch::X64, false).ffmpeg_archives();
        assert_eq!(w.len(), 1);
        assert!(w[0].url.ends_with("ffmpeg-master-latest-win64-gpl.zip"));
        assert_eq!(w[0].kind, ArchiveKind::Zip);
        assert_eq!(w[0].files, ["ffmpeg.exe", "ffprobe.exe"]);

        let l = t(Os::Linux, Arch::Arm64, false).ffmpeg_archives();
        assert!(
            l[0].url
                .ends_with("ffmpeg-master-latest-linuxarm64-gpl.tar.xz")
        );
        assert_eq!(l[0].kind, ArchiveKind::TarXz);

        let m = t(Os::MacOs, Arch::Arm64, false).ffmpeg_archives();
        assert_eq!(m.len(), 2);
        assert_eq!(
            m[1].url,
            "https://evermeet.cx/ffmpeg/getrelease/ffprobe/zip"
        );

        assert!(t(Os::Linux, Arch::X64, true).ffmpeg_archives().is_empty());
        assert!(
            t(Os::Linux, Arch::Armv7, false)
                .ffmpeg_archives()
                .is_empty()
        );
        assert!(
            t(Os::Windows, Arch::X86, false)
                .ffmpeg_archives()
                .is_empty()
        );
    }

    #[test]
    fn describe_and_exe_names() {
        assert_eq!(t(Os::Linux, Arch::X64, true).describe(), "linux(musl)-x64");
        assert_eq!(
            t(Os::Windows, Arch::Arm64, false).exe_name("deno"),
            "deno.exe"
        );
        assert_eq!(t(Os::Linux, Arch::X64, false).exe_name("deno"), "deno");
    }
}
