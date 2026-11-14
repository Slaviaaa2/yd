//! Interactive mode (phase 2). Stub: the REPL worker replaces this file.

use crate::config::Config;
use crate::tools::ToolOptions;

/// Entry point for `yd` without URLs. Returns the process exit code.
pub fn run(_config: Config, _tool_opts: ToolOptions) -> anyhow::Result<u8> {
    println!("interactive mode: coming soon");
    Ok(0)
}
