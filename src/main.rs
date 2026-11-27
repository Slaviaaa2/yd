mod cli;
mod clipboard;
mod config;
mod format;
mod repl;
mod temp_sh;
mod tools;
mod ui;
mod upgrade;
mod ytdlp;

use std::io::{self, IsTerminal, Write};
use std::time::Duration;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use clap::Parser;

use cli::{Cli, Command, FavAction};
use config::Config;
use format::FormatSpec;
use tools::ToolOptions;

const EXIT_USAGE: u8 = 1;
const EXIT_FAIL: u8 = 2;
const EXIT_INTERRUPTED: u8 = 130;

fn main() -> ExitCode {
    ui::init();
    #[cfg(windows)]
    upgrade::clean_old_executable();
    let cli = match Cli::try_parse() {
        Ok(c) => c,
        Err(e) => {
            let code = if e.use_stderr() { EXIT_USAGE } else { 0 };
            let _ = e.print();
            return ExitCode::from(code);
        }
    };
    match real_main(cli) {
        Ok(code) => ExitCode::from(code),
        Err(e) => {
            eprintln!("{}", ui::error_line(&format!("{e:#}")));
            ExitCode::from(EXIT_FAIL)
        }
    }
}

fn usage_error(msg: &str) -> u8 {
    eprintln!("{}", ui::error_line(msg));
    EXIT_USAGE
}

fn load_config() -> Config {
    let (cfg, warning) = Config::load();
    if let Some(w) = warning {
        println!("{}", ui::warn_line(&w));
    }
    cfg
}

fn real_main(cli: Cli) -> anyhow::Result<u8> {
    match cli.command {
        Some(Command::Fav { action }) => return fav(action),
        Some(Command::Update { no_deno }) => return update(no_deno),
        Some(Command::Upgrade) => return upgrade_cli(false),
        Some(Command::Config) => return show_config(),
        None => {}
    }
    if cli.pick {
        if cli.urls.is_empty() {
            return Ok(usage_error("--pick needs at least one URL (interactive mode has /f and /o)"));
        }
        if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
            return Ok(usage_error("--pick needs an interactive terminal"));
        }
    }

    if interactive_terminal()
        && let Ok(Some(next)) = upgrade::check(Duration::from_secs(2))
        && let Err(e) = offer_upgrade(&next)
    {
        eprintln!("{}", ui::warn_line(&format!("{e:#}")));
    }

    let mut cfg = load_config();
    let tool_opts = ToolOptions { skip_deno: cli.no_deno };
    if cli.urls.is_empty() {
        return repl::run(cfg, tool_opts);
    }

    // Resolve format / destination / template.
    let spec = match cli.format.as_deref().or(cfg.last_format.as_deref()) {
        Some(s) => match FormatSpec::parse(s) {
            Ok(f) => f,
            Err(e) => return Ok(usage_error(&e)),
        },
        None => FormatSpec::DEFAULT,
    };
    let mut spec = spec;
    let mut upload = cli.out.as_deref().is_some_and(config::is_temp_sh_dest);
    let mut dest = match cli.out.as_deref() {
        Some(o) if !upload => match cfg.resolve_dest(o) {
            Ok(d) => d,
            Err(e) => return Ok(usage_error(&format!("{e:#}"))),
        },
        _ => cfg.default_dest(),
    };
    if cli.pick {
        let Some(picked) = ui::picker::pick_format(&spec)? else {
            eprintln!("{}", ui::error_line("Cancelled"));
            return Ok(EXIT_INTERRUPTED);
        };
        spec = picked;
    }
    if cli.out.is_none() && (cli.pick || (cfg.settings.ask_save_method && interactive_terminal())) {
        let Some(picked) = ui::picker::pick_dest(&cfg, &dest)? else {
            eprintln!("{}", ui::error_line("Cancelled"));
            return Ok(EXIT_INTERRUPTED);
        };
        let label = match picked {
            ui::picker::DestChoice::Local(local) => {
                dest = local;
                ui::abbrev_home(&dest)
            }
            ui::picker::DestChoice::TempSh => {
                upload = true;
                "temp.sh".into()
            }
        };
        println!("{}", ui::dim(format!("  {spec} \u{2192} {label}")));
    }
    let template =
        cli.template.clone().or_else(|| cfg.template.clone()).unwrap_or_else(|| format::DEFAULT_TEMPLATE.to_string());

    let cancel = Arc::new(AtomicBool::new(false));
    {
        let cancel = cancel.clone();
        ctrlc::set_handler(move || cancel.store(true, Ordering::SeqCst))?;
    }

    let mut tp = ui::oneshot::ToolPrinter::new(false);
    let tools = tools::ensure_tools(&tool_opts, &mut |e| tp.event(e));
    tp.finish();
    let tools = tools?;

    let built = match spec.build_args(tools.ffmpeg_dir.is_some()) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("{}", ui::error_line(&e));
            return Ok(EXIT_FAIL);
        }
    };
    for w in &built.warnings {
        println!("{}", ui::warn_line(w));
    }

    let mut printer = ui::oneshot::Printer::new(spec.to_string());
    let mut failed = false;
    for url in &cli.urls {
        let job = ytdlp::Job {
            url: url.clone(),
            upload,
            dest: dest.clone(),
            template: template.clone(),
            force: cli.force,
            playlist: cli.playlist,
            cookies_from_browser: cli.cookies.clone(),
            extra: cli.passthrough.clone(),
        };
        let outcome = ytdlp::run(&job, &built, &tools, &cancel, |ev| printer.event(ev))?;
        printer.finish();
        if outcome.cancelled || cancel.load(Ordering::SeqCst) {
            eprintln!("{}", ui::error_line("Interrupted"));
            return Ok(EXIT_INTERRUPTED);
        }
        if outcome.success() || !outcome.saved.is_empty() || !outcome.already.is_empty() {
            if upload {
                cfg.last_format = Some(spec.canonical());
            } else {
                cfg.record_download(&dest, &spec.canonical());
            }
            if let Err(e) = cfg.save() {
                println!("{}", ui::warn_line(&format!("cannot save config: {e:#}")));
            }
        }
        if !outcome.success() {
            failed = true;
        }
    }
    Ok(if failed { EXIT_FAIL } else { 0 })
}

fn fav(action: FavAction) -> anyhow::Result<u8> {
    let mut cfg = load_config();
    match action {
        FavAction::Add { name, path } => {
            let p = cfg.add_favorite(&name, path.as_deref().unwrap_or("."))?;
            cfg.save()?;
            println!("{}", ui::ok_line(&format!("Favorite {name} {}", ui::abbrev_home(&p))));
        }
        FavAction::Rm { name } => {
            if cfg.remove_favorite(&name) {
                cfg.save()?;
                println!("{}", ui::ok_line(&format!("Removed favorite {name}")));
            } else {
                return Ok(usage_error(&format!("no favorite named '{name}'")));
            }
        }
        FavAction::Ls => {
            for l in ui::fav_list_lines(&cfg) {
                println!("{l}");
            }
        }
    }
    Ok(0)
}

fn update(no_deno: bool) -> anyhow::Result<u8> {
    let mut tp = ui::oneshot::ToolPrinter::new(true);
    let r = tools::update_tools(&ToolOptions { skip_deno: no_deno }, &mut |e| tp.event(e));
    tp.finish();
    if let Err(e) = &r {
        eprintln!("{}", ui::error_line(&format!("{e:#}")));
    } else {
        println!("{}", ui::ok_line("Tools are up to date"));
    }
    let code = upgrade_cli(true)?;
    Ok(if r.is_err() { EXIT_FAIL } else { code })
}

fn interactive_terminal() -> bool {
    io::stdin().is_terminal() && io::stdout().is_terminal()
}

fn offer_upgrade(next: &upgrade::Upgrade) -> anyhow::Result<()> {
    if !interactive_terminal() {
        println!("{}", ui::heading(&format!("{} Run yd upgrade to install.", next.question())));
        return Ok(());
    }
    print!("{} [y/N] ", next.question());
    io::stdout().flush()?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    if matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
        install_upgrade(next)?;
    }
    Ok(())
}

fn install_upgrade(next: &upgrade::Upgrade) -> anyhow::Result<()> {
    let mut printer = ui::oneshot::ToolPrinter::with_title("Upgrading yd", false);
    let result = next.install(|event| printer.event(event));
    printer.finish();
    println!("{}", ui::ok_line(&result?));
    Ok(())
}

fn upgrade_cli(confirm: bool) -> anyhow::Result<u8> {
    match upgrade::check(upgrade::CHECK_TIMEOUT)? {
        Some(next) if confirm => offer_upgrade(&next)?,
        Some(next) => install_upgrade(&next)?,
        None => println!("{}", ui::ok_line("yd is up to date")),
    }
    Ok(0)
}

fn show_config() -> anyhow::Result<u8> {
    let path = config::config_path();
    println!("{}", ui::heading(&ui::abbrev_home(&path)));
    match std::fs::read_to_string(&path) {
        Ok(t) if t.trim().is_empty() => println!("{}", ui::detail("(empty)")),
        Ok(t) => println!("{t}"),
        Err(_) => println!("{}", ui::detail("(not created yet)")),
    }
    Ok(0)
}
