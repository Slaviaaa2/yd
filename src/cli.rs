//! Command line definition.

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "yd",
    version,
    about = "Fast, friendly yt-dlp front-end",
    args_conflicts_with_subcommands = true
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,

    /// URLs to download (none: interactive mode)
    #[arg(value_name = "URL")]
    pub urls: Vec<String>,

    /// Format: mp3, mp3:320, m4a, flac, mp4:1080, mkv:4k, gif:360, original ...
    #[arg(short, long, value_name = "SPEC")]
    pub format: Option<String>,

    /// Destination: favorite name, f:N / r:N (Nth favorite / recent), or directory (created if missing)
    #[arg(short, long, value_name = "DEST")]
    pub out: Option<String>,

    /// Pick format and destination interactively
    #[arg(short, long)]
    pub pick: bool,

    /// Output template override
    #[arg(short, long, value_name = "T")]
    pub template: Option<String>,

    /// Overwrite existing files (default: skip)
    #[arg(long)]
    pub force: bool,

    /// Download the whole playlist for URLs with both v= and list=
    #[arg(long)]
    pub playlist: bool,

    /// Load cookies from a browser (e.g. firefox, chrome)
    #[arg(long, value_name = "BROWSER")]
    pub cookies: Option<String>,

    /// Do not set up deno
    #[arg(long)]
    pub no_deno: bool,

    /// Extra arguments passed to yt-dlp as-is
    #[arg(last = true, value_name = "ARGS")]
    pub passthrough: Vec<String>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Manage favorite destinations
    Fav {
        #[command(subcommand)]
        action: FavAction,
    },
    /// Update helper tools and check for a new yd version
    Update {
        /// Do not set up deno
        #[arg(long)]
        no_deno: bool,
    },
    /// Install the latest published yd version
    Upgrade,
    /// Print config file path and contents
    Config,
}

#[derive(Debug, Subcommand)]
pub enum FavAction {
    /// Add or replace a favorite (path defaults to the current directory)
    Add { name: String, path: Option<String> },
    /// Remove a favorite
    Rm { name: String },
    /// List favorites and recent directories
    Ls,
}
