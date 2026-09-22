pub mod api;
pub mod app;
pub mod backup;
pub mod cache;
pub mod cli;
pub mod clipboard;
pub mod config;
pub mod interactions;
pub mod keychain;
pub mod phase_c;
pub mod reminders;
pub mod ui;
pub mod view_cache;

use anyhow::Context;
use crossterm::event::{DisableBracketedPaste, EnableBracketedPaste};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io::stdout;
use tracing_subscriber::EnvFilter;

use crate::client::app::App;
use crate::client::config::Config;

/// File logging for the TUI / agent CLI (stdout stays clean).
pub fn init_logging() -> anyhow::Result<()> {
    let log_path = config::data_dir()?.join("conr-client.log");
    if let Some(parent) = log_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .ok();
    if let Some(file) = file {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(
                EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
            )
            .with_writer(file)
            .with_ansi(false)
            .try_init();
    }
    Ok(())
}

/// Interactive TUI (alternate screen).
pub async fn run_tui() -> anyhow::Result<()> {
    init_logging()?;
    let config = Config::load().context("load config")?;
    let mut app = App::new(config).await?;

    enable_raw_mode()?;
    let mut out = stdout();
    execute!(out, EnterAlternateScreen, EnableBracketedPaste)?;
    let backend = CrosstermBackend::new(out);
    let mut terminal = Terminal::new(backend)?;

    let result = app.run(&mut terminal).await;

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableBracketedPaste
    )?;
    terminal.show_cursor()?;

    if let Err(e) = result {
        eprintln!("conr error: {e:#}");
        return Err(e);
    }
    Ok(())
}

/// Agent-facing subcommand CLI (JSON on stdout).
pub async fn run_agent(cli: cli::Cli) -> anyhow::Result<()> {
    init_logging()?;
    cli.run().await
}

/// Classic entry: no args → TUI, args → agent CLI (used by the thin bin).
pub async fn run() -> anyhow::Result<()> {
    if std::env::args_os().len() <= 1 {
        return run_tui().await;
    }
    let cli = cli::Cli::parse();
    run_agent(cli).await
}
