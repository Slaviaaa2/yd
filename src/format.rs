//! Format spec `<kind>[:<quality>]` -> yt-dlp arguments.
//!
//! Audio: mp3 m4a aac opus ogg flac wav alac aiff (quality = kbps or `best`).
//! Video: mp4 mkv webm mov avi gif (quality = height, `4k`, `2k` or `best`).
//! `original` / `best`: no conversion.

#![allow(dead_code)] // public API also used by the interactive mode

use std::fmt;

/// Default output template. Playlist entries go into a `<playlist title>/` subfolder
/// (`./` for single videos). `%(playlist_title&{}/|)s` does not work: yt-dlp sanitizes its `/`.
pub const DEFAULT_TEMPLATE: &str = "%(playlist_title|.)s/%(uploader,channel|Unknown)s - %(title)s.%(ext)s";

/// Selectable bitrates for audio pickers (kbps).
pub const AUDIO_KBPS: [u32; 4] = [320, 256, 192, 128];
/// Selectable heights for video pickers.
pub const VIDEO_HEIGHTS: [u32; 6] = [2160, 1440, 1080, 720, 480, 360];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Container {
    Original,
    Mp3,
    M4a,
    Aac,
    Opus,
    Ogg,
    Flac,
    Wav,
    Alac,
    Aiff,
    Mp4,
    Mkv,
    Webm,
    Mov,
    Avi,
    Gif,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quality {
    Best,
    /// Audio bitrate in kbps.
    Kbps(u32),
    /// Video height in pixels.
    Height(u32),
}

impl Container {
    pub const AUDIO: [Container; 9] = [
        Container::Mp3,
        Container::M4a,
        Container::Aac,
        Container::Opus,
        Container::Ogg,
        Container::Flac,
        Container::Wav,
        Container::Alac,
        Container::Aiff,
    ];
    pub const VIDEO: [Container; 6] = [
        Container::Mp4,
        Container::Mkv,
        Container::Webm,
        Container::Mov,
        Container::Avi,
        Container::Gif,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Container::Original => "original",
            Container::Mp3 => "mp3",
            Container::M4a => "m4a",
            Container::Aac => "aac",
            Container::Opus => "opus",
            Container::Ogg => "ogg",
            Container::Flac => "flac",
            Container::Wav => "wav",
            Container::Alac => "alac",
            Container::Aiff => "aiff",
            Container::Mp4 => "mp4",
            Container::Mkv => "mkv",
            Container::Webm => "webm",
            Container::Mov => "mov",
            Container::Avi => "avi",
            Container::Gif => "gif",
        }
    }

    pub fn is_audio(self) -> bool {
        Self::AUDIO.contains(&self)
    }

    pub fn is_video(self) -> bool {
        Self::VIDEO.contains(&self)
    }

    /// Lossless/PCM containers ignore quality.
    pub fn is_lossless(self) -> bool {
        matches!(
            self,
            Container::Flac | Container::Wav | Container::Alac | Container::Aiff
        )
    }

    /// Whether yt-dlp can embed a thumbnail as cover art into this container.
    fn supports_thumbnail(self) -> bool {
        matches!(
            self,
            Container::Mp3
                | Container::M4a
                | Container::Opus
                | Container::Ogg
                | Container::Flac
                | Container::Alac
                | Container::Mp4
                | Container::Mkv
        )
    }

    /// Qualities offered by pickers for this container (always starts with Best).
    pub fn qualities(self) -> Vec<Quality> {
        let mut v = vec![Quality::Best];
        if self.is_video() && self != Container::Gif {
            v.extend(VIDEO_HEIGHTS.iter().map(|&h| Quality::Height(h)));
        } else if self == Container::Gif {
            v.extend([480, 360, 240].iter().map(|&h| Quality::Height(h)));
        } else if self.is_audio() && !self.is_lossless() {
            v.extend(AUDIO_KBPS.iter().map(|&k| Quality::Kbps(k)));
        }
        v
    }
}

impl fmt::Display for Quality {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Quality::Best => write!(f, "best"),
            Quality::Kbps(k) => write!(f, "{k}k"),
            Quality::Height(h) => write!(f, "{h}p"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FormatSpec {
    pub container: Container,
    pub quality: Quality,
}

impl FormatSpec {
    pub const ORIGINAL: FormatSpec = FormatSpec {
        container: Container::Original,
        quality: Quality::Best,
    };
    pub const DEFAULT: FormatSpec = FormatSpec {
        container: Container::Mp4,
        quality: Quality::Best,
    };

    pub fn new(container: Container, quality: Quality) -> Self {
        FormatSpec { container, quality }
    }

    /// Parse `mp3`, `.mp3:320`, `mp4:1080`, `mp4:4k`, `original`, ...
    pub fn parse(input: &str) -> Result<FormatSpec, String> {
        let s = input.trim().trim_start_matches('.').to_ascii_lowercase();
        let (kind, quality) = match s.split_once(':') {
            Some((k, q)) => (k, Some(q)),
            None => (s.as_str(), None),
        };
        let container = match kind {
            "original" | "best" => Container::Original,
            "mp3" => Container::Mp3,
            "m4a" => Container::M4a,
            "aac" => Container::Aac,
            "opus" => Container::Opus,
            "ogg" | "vorbis" => Container::Ogg,
            "flac" => Container::Flac,
            "wav" => Container::Wav,
            "alac" => Container::Alac,
            "aiff" | "aif" => Container::Aiff,
            "mp4" => Container::Mp4,
            "mkv" => Container::Mkv,
            "webm" => Container::Webm,
            "mov" => Container::Mov,
            "avi" => Container::Avi,
            "gif" => Container::Gif,
            _ => {
                return Err(format!(
                    "unknown format '{kind}' (try mp3, m4a, flac, mp4, mkv, original ...)"
                ));
            }
        };
        let quality = match quality {
            None | Some("") | Some("best") => Quality::Best,
            Some(q) => {
                if container == Container::Original {
                    return Err("'original' takes no quality".into());
                }
                if container.is_lossless() {
                    Quality::Best
                } else if container.is_audio() {
                    let n = q.strip_suffix('k').unwrap_or(q);
                    match n.parse::<u32>() {
                        Ok(k) if (8..=640).contains(&k) => Quality::Kbps(k),
                        _ => return Err(format!("bad audio quality '{q}' (kbps like 320, 128, or best)")),
                    }
                } else {
                    match q {
                        "4k" => Quality::Height(2160),
                        "2k" => Quality::Height(1440),
                        _ => {
                            let n = q.strip_suffix('p').unwrap_or(q);
                            match n.parse::<u32>() {
                                Ok(h) if (100..=8640).contains(&h) => Quality::Height(h),
                                _ => return Err(format!("bad video quality '{q}' (height like 1080, 4k, or best)")),
                            }
                        }
                    }
                }
            }
        };
        Ok(FormatSpec { container, quality })
    }

    /// Compact form that round-trips through `parse` (stored in config).
    pub fn canonical(&self) -> String {
        match self.quality {
            Quality::Best => self.container.name().to_string(),
            Quality::Kbps(k) => format!("{}:{k}", self.container.name()),
            Quality::Height(h) => format!("{}:{h}", self.container.name()),
        }
    }

    /// File extension the result is expected to have (None for original).
    pub fn ext(&self) -> Option<&'static str> {
        match self.container {
            Container::Original => None,
            Container::Aac => Some("m4a"),
            Container::Alac => Some("m4a"),
            c => Some(c.name()),
        }
    }

    pub fn needs_ffmpeg(&self) -> bool {
        self.container != Container::Original
    }

    /// Every sensible choice for pickers, in display order.
    pub fn all_choices() -> Vec<FormatSpec> {
        let mut v = vec![FormatSpec::ORIGINAL];
        for c in Container::AUDIO.iter().chain(Container::VIDEO.iter()) {
            for q in c.qualities() {
                v.push(FormatSpec::new(*c, q));
            }
        }
        v
    }

    /// One entry per container (quality Best), for the first level of a picker.
    pub fn containers() -> Vec<Container> {
        let mut v = vec![Container::Original];
        v.extend(Container::AUDIO);
        v.extend(Container::VIDEO);
        v
    }

    /// Build yt-dlp format/postprocessing arguments.
    pub fn build_args(&self, has_ffmpeg: bool) -> Result<BuiltArgs, String> {
        let mut args: Vec<String> = Vec::new();
        let mut warnings = Vec::new();
        let c = self.container;

        if !has_ffmpeg {
            match c {
                Container::Original => add(&mut args, &["-f", "b"]),
                c if c.is_video() => {
                    match self.quality {
                        Quality::Height(h) => {
                            let f = format!("b[height<={h}]/b");
                            args.extend(["-f".to_string(), f]);
                        }
                        _ => add(&mut args, &["-f", "b"]),
                    }
                    warnings.push(format!(
                        "ffmpeg not available: downloading a single file, {} conversion skipped",
                        c.name()
                    ));
                }
                c => {
                    return Err(format!(
                        "converting to {} needs ffmpeg, which is not available (use `original` to keep the source audio)",
                        c.name()
                    ));
                }
            }
            return Ok(BuiltArgs { args, warnings });
        }

        match c {
            Container::Original => {
                add(&mut args, &["-f", "bv*+ba/b", "--embed-metadata", "--embed-chapters"]);
            }
            c if c.is_audio() => {
                add(&mut args, &["-f", "ba/b"]);
                let q = match self.quality {
                    Quality::Kbps(k) if !c.is_lossless() => format!("{k}K"),
                    _ => "0".to_string(),
                };
                match c {
                    Container::Aiff => {
                        add(
                            &mut args,
                            &[
                                "--recode-video",
                                "aiff",
                                "--postprocessor-args",
                                "VideoConvertor+ffmpeg_o:-vn",
                            ],
                        );
                    }
                    Container::Alac => {
                        // `-x --audio-format alac` alone yields AAC; force the codec.
                        add(
                            &mut args,
                            &[
                                "-x",
                                "--audio-format",
                                "alac",
                                "--postprocessor-args",
                                "ExtractAudio+ffmpeg_o:-c:a alac",
                            ],
                        );
                    }
                    Container::Ogg => {
                        add(&mut args, &["-x", "--audio-format", "vorbis", "--audio-quality", &q]);
                    }
                    c if c.is_lossless() => add(&mut args, &["-x", "--audio-format", c.name()]),
                    c => add(&mut args, &["-x", "--audio-format", c.name(), "--audio-quality", &q]),
                }
                add(&mut args, &["--embed-metadata"]);
                if c.supports_thumbnail() {
                    add(&mut args, &["--embed-thumbnail"]);
                }
            }
            c => {
                add(&mut args, &["-f", "bv*+ba/b"]);
                let res = match self.quality {
                    Quality::Height(h) => format!("res:{h}"),
                    _ => "res".to_string(),
                };
                match c {
                    Container::Mp4 | Container::Mov => {
                        args.extend(["-S".to_string(), format!("{res},vcodec:h264,acodec:aac")]);
                        add(
                            &mut args,
                            &["--merge-output-format", c.name(), "--remux-video", c.name()],
                        );
                    }
                    Container::Mkv => {
                        args.extend(["-S".to_string(), res]);
                        add(&mut args, &["--merge-output-format", "mkv", "--remux-video", "mkv"]);
                    }
                    Container::Webm => {
                        args.extend(["-S".to_string(), format!("{res},vcodec:vp9,acodec:opus")]);
                        add(&mut args, &["--merge-output-format", "webm", "--remux-video", "webm"]);
                    }
                    Container::Avi => {
                        args.extend(["-S".to_string(), format!("{res},vcodec:h264,acodec:aac")]);
                        add(&mut args, &["--recode-video", "avi"]);
                    }
                    Container::Gif => {
                        args.extend(["-S".to_string(), res]);
                        // Drop audio (it breaks the GIF encoder) and use a palette for decent colors.
                        add(
                            &mut args,
                            &[
                                "--recode-video",
                                "gif",
                                "--postprocessor-args",
                                "VideoConvertor+ffmpeg_o:-an -vf \"fps=12,split[a][b];[a]palettegen[p];[b][p]paletteuse\"",
                            ],
                        );
                    }
                    _ => unreachable!(),
                }
                add(&mut args, &["--embed-metadata"]);
                if c.supports_thumbnail() {
                    add(&mut args, &["--embed-thumbnail"]);
                }
                if matches!(c, Container::Mp4 | Container::Mkv | Container::Webm | Container::Mov) {
                    add(&mut args, &["--embed-chapters"]);
                }
            }
        }
        Ok(BuiltArgs { args, warnings })
    }
}

fn add(args: &mut Vec<String>, xs: &[&str]) {
    args.extend(xs.iter().map(|s| s.to_string()));
}

#[derive(Debug, Clone)]
pub struct BuiltArgs {
    pub args: Vec<String>,
    pub warnings: Vec<String>,
}

impl fmt::Display for FormatSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let c = self.container;
        if c == Container::Original || c.is_lossless() {
            write!(f, "{}", c.name())
        } else {
            write!(f, "{} \u{b7} {}", c.name(), self.quality)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> FormatSpec {
        FormatSpec::parse(s).unwrap()
    }

    #[test]
    fn parses_audio() {
        assert_eq!(p("mp3"), FormatSpec::new(Container::Mp3, Quality::Best));
        assert_eq!(p(".MP3:320"), FormatSpec::new(Container::Mp3, Quality::Kbps(320)));
        assert_eq!(p("opus:128k"), FormatSpec::new(Container::Opus, Quality::Kbps(128)));
        assert_eq!(p("vorbis").container, Container::Ogg);
        assert_eq!(p("flac:320").quality, Quality::Best);
    }

    #[test]
    fn parses_video() {
        assert_eq!(p("mp4:1080"), FormatSpec::new(Container::Mp4, Quality::Height(1080)));
        assert_eq!(p("mkv:4k").quality, Quality::Height(2160));
        assert_eq!(p("webm:2K").quality, Quality::Height(1440));
        assert_eq!(p("mp4:720p").quality, Quality::Height(720));
        assert_eq!(p("gif:best").quality, Quality::Best);
    }

    #[test]
    fn parses_original() {
        assert_eq!(p("original"), FormatSpec::ORIGINAL);
        assert_eq!(p("best"), FormatSpec::ORIGINAL);
        assert!(FormatSpec::parse("original:720").is_err());
    }

    #[test]
    fn rejects_bad() {
        assert!(FormatSpec::parse("exe").is_err());
        assert!(FormatSpec::parse("mp3:fast").is_err());
        assert!(FormatSpec::parse("mp4:abc").is_err());
        assert!(FormatSpec::parse("mp3:1").is_err());
    }

    #[test]
    fn display() {
        assert_eq!(p("mp3:320").to_string(), "mp3 \u{b7} 320k");
        assert_eq!(p("mp4:1080").to_string(), "mp4 \u{b7} 1080p");
        assert_eq!(p("original").to_string(), "original");
        assert_eq!(p("flac").to_string(), "flac");
    }

    #[test]
    fn canonical_roundtrip() {
        for s in FormatSpec::all_choices() {
            assert_eq!(FormatSpec::parse(&s.canonical()).unwrap(), s);
        }
    }

    #[test]
    fn mp3_args() {
        let a = p("mp3:320").build_args(true).unwrap().args;
        let j = a.join(" ");
        assert!(j.contains("-x --audio-format mp3 --audio-quality 320K"), "{j}");
        assert!(j.contains("--embed-thumbnail"));
        let a = p("mp3").build_args(true).unwrap().args.join(" ");
        assert!(a.contains("--audio-quality 0"));
    }

    #[test]
    fn wav_has_no_thumbnail() {
        let a = p("wav").build_args(true).unwrap().args.join(" ");
        assert!(!a.contains("--embed-thumbnail"));
        assert!(a.contains("--audio-format wav"));
        assert!(!a.contains("--audio-quality"));
    }

    #[test]
    fn mp4_args_keep_res_first() {
        let a = p("mp4:1080").build_args(true).unwrap().args.join(" ");
        assert!(a.contains("-S res:1080,vcodec:h264,acodec:aac"), "{a}");
        assert!(a.contains("--merge-output-format mp4"));
        assert!(a.contains("--embed-chapters"));
        let a = p("mp4").build_args(true).unwrap().args.join(" ");
        assert!(a.contains("-S res,vcodec:h264,acodec:aac"), "{a}");
    }

    #[test]
    fn recode_formats() {
        assert!(
            p("gif:360")
                .build_args(true)
                .unwrap()
                .args
                .join(" ")
                .contains("--recode-video gif")
        );
        assert!(
            p("aiff")
                .build_args(true)
                .unwrap()
                .args
                .join(" ")
                .contains("--recode-video aiff")
        );
        assert!(
            !p("avi")
                .build_args(true)
                .unwrap()
                .args
                .join(" ")
                .contains("--embed-thumbnail")
        );
    }

    #[test]
    fn no_ffmpeg() {
        let b = p("mp4:720").build_args(false).unwrap();
        assert_eq!(b.args, ["-f", "b[height<=720]/b"]);
        assert_eq!(b.warnings.len(), 1);
        assert!(p("mp3").build_args(false).is_err());
        assert_eq!(p("original").build_args(false).unwrap().args, ["-f", "b"]);
    }
}
