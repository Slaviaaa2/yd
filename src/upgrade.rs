//! Discover published yd releases and replace the resolved executable in place.

use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, ensure};
use semver::Version;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::tools::ToolEvent;
use crate::tools::download::{self, TempGuard};
use crate::tools::platform::{Arch, ArchiveKind, Os, Target};

pub const CHECK_TIMEOUT: Duration = Duration::from_secs(10);

const REPO: &str = "https://github.com/Slaviaaa2/yd";
const LATEST: &str = "https://api.github.com/repos/Slaviaaa2/yd/releases/latest";

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    digest: Option<String>,
}

pub struct Upgrade {
    version: Version,
    url: String,
    digest: String,
    kind: ArchiveKind,
    binary: String,
}

pub struct InstalledUpgrade {
    exe: PathBuf,
    version: Version,
}

impl InstalledUpgrade {
    pub fn message(&self) -> String {
        format!("Upgraded yd to v{}.", self.version)
    }

    /// Use the path resolved before replacement. On Linux the old running image is now deleted.
    pub fn restart(self) -> Result<u8> {
        let mut command = std::process::Command::new(&self.exe);
        command.args(std::env::args_os().skip(1));
        let result: io::Result<u8>;
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            result = Err(command.exec());
        }
        #[cfg(not(unix))]
        {
            result = command.status().map(|status| {
                status
                    .code()
                    .and_then(|code| u8::try_from(code).ok())
                    .unwrap_or(2)
            });
        }
        result.with_context(|| format!("cannot restart {}", self.exe.display()))
    }
}

impl Upgrade {
    pub fn question(&self) -> String {
        format!("yd v{} is available. Upgrade?", self.version)
    }

    pub fn install(&self, mut report: impl FnMut(ToolEvent)) -> Result<InstalledUpgrade> {
        let exe = executable_path()?;
        let parent = exe
            .parent()
            .context("yd executable has no parent directory")?;
        let lock = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(parent.join(".yd-upgrade.lock"))
            .with_context(|| format!("cannot write to {}", parent.display()))?;
        lock.try_lock()
            .context("cannot lock the yd installation for upgrade")?;
        let stage = download::temp_path(parent, ".yd-upgrade", "stage");
        fs::create_dir(&stage).context("cannot create upgrade staging directory")?;
        let stage = TempGuard::new(stage);
        let archive = stage.path().join("release");
        download::download_file(&self.url, &archive, &mut |done, total| {
            report(ToolEvent::Downloading {
                name: "yd",
                done,
                total,
            });
        })?;

        let mut hasher = Sha256::new();
        io::copy(&mut File::open(&archive)?, &mut hasher)?;
        ensure!(
            format!("sha256:{:x}", hasher.finalize()) == self.digest,
            "yd release archive checksum mismatch"
        );

        let extracted = download::extract_files(
            &archive,
            self.kind,
            std::slice::from_ref(&self.binary),
            stage.path(),
        )?;
        ensure!(
            !extracted.is_empty(),
            "yd executable not found in release archive"
        );
        let staged = stage.path().join(&self.binary);
        OpenOptions::new().write(true).open(&staged)?.sync_all()?;
        replace(&staged, &exe).with_context(|| format!("cannot replace {}", exe.display()))?;
        Ok(InstalledUpgrade {
            exe,
            version: self.version.clone(),
        })
    }
}

fn executable_path() -> Result<PathBuf> {
    std::env::current_exe()?
        .canonicalize()
        .context("cannot resolve the yd executable path")
}

#[cfg(not(windows))]
fn replace(staged: &Path, exe: &Path) -> Result<()> {
    fs::rename(staged, exe)?;
    Ok(())
}

#[cfg(windows)]
fn backup_path(exe: &Path) -> PathBuf {
    let mut backup = exe.as_os_str().to_os_string();
    backup.push(".yd-upgrade-old");
    PathBuf::from(backup)
}

#[cfg(windows)]
fn replace(staged: &Path, exe: &Path) -> Result<()> {
    let backup = backup_path(exe);
    remove_backup(&backup).context("cannot remove previous executable backup")?;
    // Windows allows the running executable to be renamed, but keeps it locked against deletion.
    fs::rename(exe, &backup)?;
    if let Err(install_error) = fs::rename(staged, exe) {
        fs::rename(&backup, exe).with_context(|| {
            format!(
                "upgrade failed ({install_error}); restore the executable from {}",
                backup.display()
            )
        })?;
        return Err(install_error.into());
    }
    Ok(())
}

#[cfg(windows)]
fn remove_backup(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

#[cfg(windows)]
pub fn clean_old_executable() {
    if let Ok(exe) = executable_path() {
        let _ = remove_backup(&backup_path(&exe));
    }
}

pub fn check(timeout: Duration) -> Result<Option<Upgrade>> {
    let target = Target::current();
    let Some((name, kind)) = asset_for(target) else {
        anyhow::bail!("no yd release build for {}", target.describe());
    };
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .user_agent(concat!("yd/", env!("CARGO_PKG_VERSION")))
        .build()
        .into();
    let reply = agent
        .get(LATEST)
        .header("Accept", "application/vnd.github+json")
        .call()
        .context("cannot check for a new yd version")?
        .body_mut()
        .with_config()
        .limit(256 * 1024)
        .read_to_string()?;
    let release = serde_json::from_str(&reply).context("invalid yd release metadata")?;
    select_upgrade(
        release,
        name,
        kind,
        target,
        &Version::parse(env!("CARGO_PKG_VERSION"))?,
    )
}

fn asset_for(target: Target) -> Option<(&'static str, ArchiveKind)> {
    let name = match (target.os, target.arch, target.musl) {
        (Os::Linux, Arch::X64, false) => "yd-linux-x64.tar.gz",
        (Os::Linux, Arch::Arm64, false) => "yd-linux-arm64.tar.gz",
        (Os::MacOs, Arch::X64, _) => "yd-macos-x64.tar.gz",
        (Os::MacOs, Arch::Arm64, _) => "yd-macos-arm64.tar.gz",
        (Os::Windows, Arch::X64, _) => return Some(("yd-windows-x64.zip", ArchiveKind::Zip)),
        _ => return None,
    };
    Some((name, ArchiveKind::TarGz))
}

fn select_upgrade(
    release: Release,
    name: &str,
    kind: ArchiveKind,
    target: Target,
    current: &Version,
) -> Result<Option<Upgrade>> {
    let tag = release.tag_name;
    let version = Version::parse(tag.strip_prefix('v').context("invalid yd release tag")?)?;
    if !version.pre.is_empty() || !version.cmp_precedence(current).is_gt() {
        return Ok(None);
    }
    let asset = release
        .assets
        .into_iter()
        .find(|a| a.name == name)
        .with_context(|| format!("yd v{version} has no {name} release asset"))?;
    let digest = asset.digest.context("yd release asset has no checksum")?;
    ensure!(
        digest
            .strip_prefix("sha256:")
            .is_some_and(|hash| hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit())),
        "invalid yd release asset checksum"
    );
    Ok(Some(Upgrade {
        version,
        url: format!("{REPO}/releases/download/{tag}/{name}"),
        digest: digest.to_ascii_lowercase(),
        kind,
        binary: target.exe_name("yd"),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_newer_published_version_and_platform_asset() {
        let target = Target {
            os: Os::Linux,
            arch: Arch::Arm64,
            musl: false,
        };
        let (name, kind) = asset_for(target).unwrap();
        let release = |tag: &str, digest: Option<String>| Release {
            tag_name: tag.into(),
            assets: vec![Asset {
                name: name.into(),
                digest,
            }],
        };
        let current = Version::parse("1.9.0").unwrap();
        let select = |release| select_upgrade(release, name, kind, target, &current);
        for tag in ["v1.8.9", "v1.9.0+packaging", "v2.0.0-rc.1"] {
            assert!(select(release(tag, None)).unwrap().is_none());
        }
        let digest = format!("sha256:{}", "a".repeat(64));
        let next = select(release("v1.10.0", Some(digest.clone())))
            .unwrap()
            .unwrap();
        assert_eq!(
            next.url,
            format!("{REPO}/releases/download/v1.10.0/yd-linux-arm64.tar.gz")
        );
        assert_eq!(next.digest, digest);
        assert!(select(release("v1.10.0", Some("invalid".into()))).is_err());
    }
}
