mod api;
mod app;
mod backup;
mod cache;
mod clipboard;
mod config;
mod keychain;
mod phase_c;
mod reminders;
mod ui;
mod view_cache;

use anyhow::Context;
use crossterm::event::{DisableBracketedPaste, EnableBracketedPaste};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io::{self, stdout};
use tracing_subscriber::EnvFilter;

use crate::app::App;
use crate::config::Config;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Log to file so TUI isn't polluted
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
        tracing_subscriber::fmt()
            .with_env_filter(
                EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
            )
            .with_writer(file)
            .with_ansi(false)
            .init();
    }

    let config = Config::load().context("load config")?;
    let mut app = App::new(config).await?;

    enable_raw_mode()?;
    let mut out = stdout();
    // No mouse capture — that blocks selecting/copying text in the terminal.
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
        eprintln!("conr-client error: {e:#}");
        return Err(e);
    }
    Ok(())
}

// silence unused import if any
#[allow(dead_code)]
fn _io() -> io::Result<()> {
    Ok(())
}
