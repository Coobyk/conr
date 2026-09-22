use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};

const TIP: &str = "\
conr client         start the TUI
conr server start   start the server in the background
conr -h             full CLI help";

#[derive(Parser, Debug)]
#[command(
    name = "conr",
    version,
    about = "conr — terminal social network (client + server in one binary)",
    after_help = "AGENT COMMANDS (JSON):\n  \
register login feed next previous replies up post like repost\n  \
thread search profile notifications dm watch\n\n\
EXAMPLES:\n  \
conr client\n  \
conr server start\n  \
conr next\n  \
conr dm @user123 send hello\n  \
conr -h",
    disable_help_subcommand = true
)]
struct Conr {
    /// Server base URL (agent commands / client)
    #[arg(long, global = true, env = "CONR_SERVER")]
    server: Option<String>,

    /// Account handle (agent commands)
    #[arg(long, global = true, env = "CONR_HANDLE")]
    handle: Option<String>,

    /// Identity token (agent commands)
    #[arg(long, global = true, env = "CONR_IDENTITY")]
    identity: Option<String>,

    #[command(subcommand)]
    command: Option<ConrCmd>,
}

#[derive(Subcommand, Debug)]
enum ConrCmd {
    /// Start the interactive TUI
    Client,
    /// Control the API server
    Server {
        #[command(subcommand)]
        action: ServerAction,
    },
    /// Agent CLI (`conr next`, `conr feed`, `conr dm …`, …)
    #[command(external_subcommand)]
    Agent(Vec<String>),
}

#[derive(Subcommand, Debug)]
enum ServerAction {
    /// Start the server in the background (pid + log under ~/.local/share/conr-server/run/)
    Start,
    /// Stop a background server
    Stop,
    /// Show whether the server is running
    Status,
    /// Run the server in the foreground (logs to stderr)
    Run,
}

fn main() -> Result<()> {
    let mut cli = Conr::parse();
    let command = cli.command.take();
    match command {
        None => {
            println!("{TIP}");
            Ok(())
        }
        Some(ConrCmd::Client) => runtime()?.block_on(conr::client::run_tui()),
        Some(ConrCmd::Server { action }) => match action {
            ServerAction::Run => {
                let guard = PidGuard::install()?;
                let rt = runtime()?;
                let result = rt.block_on(conr::server::run());
                drop(guard);
                result
            }
            ServerAction::Start => server_start(),
            ServerAction::Stop => server_stop(),
            ServerAction::Status => server_status(),
        },
        Some(ConrCmd::Agent(args)) => {
            let argv = rebuild_client_argv(&cli, args);
            let agent = <conr::client::cli::Cli as clap::Parser>::parse_from(argv);
            runtime()?.block_on(conr::client::run_agent(agent))
        }
    }
}

fn runtime() -> Result<tokio::runtime::Runtime> {
    tokio::runtime::Runtime::new().context("tokio runtime")
}

fn rebuild_client_argv(globals: &Conr, rest: Vec<String>) -> Vec<String> {
    let mut argv = vec!["conr".to_string()];
    if let Some(s) = &globals.server {
        argv.push("--server".into());
        argv.push(s.clone());
    }
    if let Some(h) = &globals.handle {
        argv.push("--handle".into());
        argv.push(h.clone());
    }
    if let Some(t) = &globals.identity {
        argv.push("--identity".into());
        argv.push(t.clone());
    }
    argv.extend(rest);
    argv
}

// --- run dir / pid --------------------------------------------------------

fn run_dir() -> Result<PathBuf> {
    let dir = dirs::data_local_dir()
        .context("no data dir")?
        .join("conr-server")
        .join("run");
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn pid_path() -> Result<PathBuf> {
    Ok(run_dir()?.join("server.pid"))
}

fn log_path() -> Result<PathBuf> {
    Ok(run_dir()?.join("server.log"))
}

fn read_pid() -> Result<Option<u32>> {
    let path = pid_path()?;
    if !path.exists() {
        return Ok(None);
    }
    Ok(fs::read_to_string(&path)?.trim().parse().ok())
}

fn pid_alive(pid: u32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Writes our pid on install; removes it on drop if still ours.
struct PidGuard {
    path: PathBuf,
    pid: u32,
}

impl PidGuard {
    fn install() -> Result<Self> {
        let path = pid_path()?;
        let pid = std::process::id();
        fs::write(&path, format!("{pid}\n"))?;
        Ok(Self { path, pid })
    }
}

impl Drop for PidGuard {
    fn drop(&mut self) {
        let still_ours = fs::read_to_string(&self.path)
            .ok()
            .and_then(|s| s.trim().parse::<u32>().ok())
            .map(|p| p == self.pid)
            .unwrap_or(false);
        if still_ours {
            let _ = fs::remove_file(&self.path);
        }
    }
}

// --- server subcommands ---------------------------------------------------

fn server_start() -> Result<()> {
    if let Some(pid) = read_pid()? {
        if pid_alive(pid) {
            bail!("server already running (pid {pid}) — `conr server status`");
        }
        let _ = fs::remove_file(pid_path()?);
    }

    let exe = std::env::current_exe().context("current_exe")?;
    let log_path = log_path()?;
    if let Some(parent) = log_path.parent() {
        fs::create_dir_all(parent)?;
    }
    // Truncate-ish marker so operators can find this boot.
    {
        let mut log = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)?;
        let _ = writeln!(log, "----- conr server start {} -----", chrono_now());
    }

    let stdout = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)?;
    let stderr = stdout.try_clone().context("clone log")?;

    // setsid → new session (survives terminal close); fall back to plain spawn.
    let child = spawn_detached(&exe, stdout, stderr).context("spawn server")?;
    let spawned_pid = child.id();
    drop(child); // detach — do not wait

    // `server run` writes its own pid file; wait briefly for it.
    let deadline = Instant::now() + Duration::from_secs(3);
    let pid = loop {
        if let Some(pid) = read_pid()? {
            if pid_alive(pid) {
                break pid;
            }
        }
        if Instant::now() >= deadline {
            // Fallback: use the child pid we spawned (correct when no fork).
            if pid_alive(spawned_pid) {
                let _ = fs::write(pid_path()?, format!("{spawned_pid}\n"));
                break spawned_pid;
            }
            bail!("server failed to start — check {}", log_path.display());
        }
        std::thread::sleep(Duration::from_millis(50));
    };

    println!("conr server started (pid {pid})");
    println!("  log  {}", log_path.display());
    println!("  pid  {}", pid_path()?.display());
    Ok(())
}

fn spawn_detached(
    exe: &std::path::Path,
    stdout: std::fs::File,
    stderr: std::fs::File,
) -> Result<std::process::Child> {
    // Try setsid(1) so closing the terminal does not SIGHUP the server.
    let setsid_ok = Command::new("setsid")
        .arg("--help")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if setsid_ok {
        let child = Command::new("setsid")
            .arg(exe)
            .args(["server", "run"])
            .stdin(Stdio::null())
            .stdout(Stdio::from(stdout.try_clone()?))
            .stderr(Stdio::from(stderr.try_clone()?))
            .spawn()?;
        return Ok(child);
    }
    let child = Command::new(exe)
        .args(["server", "run"])
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .spawn()?;
    Ok(child)
}

fn server_stop() -> Result<()> {
    let Some(pid) = read_pid()? else {
        bail!("server is not running (no pid file)");
    };
    if !pid_alive(pid) {
        let _ = fs::remove_file(pid_path()?);
        bail!("stale pid {pid} cleared — server was not running");
    }
    let status = Command::new("kill")
        .args(["-TERM", &pid.to_string()])
        .status()
        .context("kill -TERM")?;
    if !status.success() {
        bail!("failed to signal pid {pid}");
    }
    for _ in 0..30 {
        if !pid_alive(pid) {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    if pid_alive(pid) {
        let _ = Command::new("kill")
            .args(["-KILL", &pid.to_string()])
            .status();
    }
    let _ = fs::remove_file(pid_path()?);
    println!("conr server stopped (pid {pid})");
    Ok(())
}

fn server_status() -> Result<()> {
    match read_pid()? {
        Some(pid) if pid_alive(pid) => {
            println!("running (pid {pid})");
            println!("  log  {}", log_path()?.display());
            Ok(())
        }
        Some(pid) => {
            println!("not running (stale pid {pid})");
            let _ = fs::remove_file(pid_path()?);
            Ok(())
        }
        None => {
            println!("not running");
            Ok(())
        }
    }
}

fn chrono_now() -> String {
    // Avoid pulling chrono into this crate — RFC-ish timestamp from system time.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("unix:{now}")
}
