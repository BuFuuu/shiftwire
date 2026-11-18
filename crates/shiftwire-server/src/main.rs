//! `shiftwire`: one binary for the daemon and the commands that control it.
//!
//! The daemon (`shiftwire run`) owns the proxy, store, API and MCP. Every other
//! command is a thin client of its local HTTP API.

mod cli;
mod client;
mod daemon;
mod guard;
mod har;
mod mcp;
mod mcp_bridge;
mod open;
mod server;
mod ui;
mod ws;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "shiftwire",
    version,
    about = "Intercepting proxy that shows you, live, what your AI pentester sends"
)]
struct Cli {
    /// Data directory (config, CA, projects, logs). Default: ~/.shiftwire
    #[arg(long, global = true, env = "SHIFTWIRE_HOME")]
    data_dir: Option<PathBuf>,
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Start shiftWire in the background (no window). Safe to run when it is already running.
    Start,
    /// Stop the background daemon gracefully.
    Stop,
    /// Stop, then start.
    Restart,
    /// Show whether shiftWire is running, its URLs and counters (default command).
    Status,
    /// Open the UI (starts shiftWire first if needed).
    Open,
    /// Run in the foreground with logs on the terminal (Ctrl+C stops it).
    Run {
        #[arg(long, hide = true)]
        detached: bool,
    },
    /// MCP over stdio for AI clients (starts shiftWire if needed). Requests are attributed to --agent.
    Mcp {
        /// Name shown on this agent's requests, e.g. claude.
        #[arg(long)]
        agent: Option<String>,
    },
    /// Show the daemon log.
    Logs {
        /// Keep printing new lines.
        #[arg(short, long)]
        follow: bool,
        /// Number of lines to show.
        #[arg(short = 'n', long, default_value_t = 50)]
        lines: usize,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    quiet_on_closed_stdout();
    match dispatch(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("shiftwire: {e:#}");
            ExitCode::FAILURE
        }
    }
}

/// `shiftwire status | head` closes stdout early; `println!` then panics. Exit
/// quietly instead, like other command-line tools.
fn quiet_on_closed_stdout() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let message = info
            .payload()
            .downcast_ref::<String>()
            .map_or("", String::as_str);
        if message.contains("failed printing to stdout") {
            std::process::exit(0);
        }
        default(info);
    }));
}

fn dispatch(cli: Cli) -> anyhow::Result<()> {
    let paths = daemon::Paths::resolve(cli.data_dir)?;
    match cli.cmd.unwrap_or(Cmd::Status) {
        Cmd::Run { detached } => server::run_daemon(&paths, detached),
        Cmd::Start => cli::start(&paths).map(drop),
        Cmd::Stop => cli::stop(&paths),
        Cmd::Restart => cli::stop(&paths).and_then(|()| cli::start(&paths).map(drop)),
        Cmd::Status => cli::status(&paths),
        Cmd::Open => cli::open(&paths),
        Cmd::Logs { follow, lines } => cli::logs(&paths, lines, follow),
        Cmd::Mcp { agent } => mcp_bridge::run(&paths, agent.as_deref()),
    }
}
