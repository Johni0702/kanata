use anyhow::{bail, Result};
use log::info;
use simplelog::*;
use std::path::{Path, PathBuf};

mod cfg;
mod custom_action;
mod kanata;
mod keys;
mod layers;
mod oskbd;
mod tcp_server;

use clap::Parser;
use kanata::Kanata;
use tcp_server::TcpServer;

type CfgPath = PathBuf;

pub struct ValidatedArgs {
    path: CfgPath,
    port: Option<i32>,
    #[cfg(target_os = "linux")]
    symlink_path: Option<String>,
}

#[derive(Parser, Debug)]
#[clap(author, version, about, verbatim_doc_comment)]
/// kanata: an advanced software key remapper
///
/// kanata remaps key presses to other keys or complex actions depending on the
/// configuration for that key. You can find the guide for creating a config
/// file here:    https://github.com/jtroo/kanata/blob/main/docs/config.adoc
///
/// If you need help, please feel welcome to create an issue or discussion in
/// the kanata repository:     https://github.com/jtroo/kanata
struct Args {
    /// Configuration file to use with kanata.
    #[clap(short, long, default_value = "kanata.kbd")]
    cfg: String,

    /// Port to run the optional TCP server on. If blank, no TCP port will be listened on.
    #[clap(short, long)]
    port: Option<i32>,

    /// Path for the symlink pointing to the newly-created device. If blank, no symlink will be
    /// created.
    #[cfg(target_os = "linux")]
    #[clap(short, long)]
    symlink_path: Option<String>,

    /// Enable debug logging.
    #[clap(short, long)]
    debug: bool,

    /// Enable trace logging; implies --debug as well.
    #[clap(short, long)]
    trace: bool,
}

/// Parse CLI arguments and initialize logging.
fn cli_init() -> Result<ValidatedArgs> {
    let args = Args::parse();

    let cfg_path = Path::new(&args.cfg);

    let log_lvl = match (args.debug, args.trace) {
        (_, true) => LevelFilter::Trace,
        (true, false) => LevelFilter::Debug,
        (false, false) => LevelFilter::Info,
    };

    let mut log_cfg = ConfigBuilder::new();
    if let Err(e) = log_cfg.set_time_offset_to_local() {
        eprintln!("WARNING: could not set log TZ to local: {:?}", e);
    };
    CombinedLogger::init(vec![TermLogger::new(
        log_lvl,
        log_cfg.build(),
        TerminalMode::Mixed,
        ColorChoice::AlwaysAnsi,
    )])
    .expect("logger can init");
    log::info!("kanata v{} starting", env!("CARGO_PKG_VERSION"));

    if !cfg_path.exists() {
        bail!(
            "Could not find your config file ({})\nFor more info, pass the `-h` or `--help` flags.",
            cfg_path.to_str().unwrap_or("?")
        )
    }

    Ok(ValidatedArgs {
        path: cfg_path.into(),
        port: args.port,
        #[cfg(target_os = "linux")]
        symlink_path: args.symlink_path,
    })
}

fn main_impl() -> Result<()> {
    let args = cli_init()?;
    let kanata_arc = Kanata::new_arc(&args)?;

    info!("Sleeping for 2s. Please release all keys and don't press additional ones.");
    std::thread::sleep(std::time::Duration::from_secs(2));

    // Start a processing loop in another thread and run the event loop in this thread.
    //
    // The reason for two different event loops is that the "event loop" only listens for keyboard
    // events, which it sends to the "processing loop". The processing loop handles keyboard events
    // while also maintaining `tick()` calls to keyberon.

    let (server, ntx, nrx) = if let Some(port) = args.port {
        let mut server = TcpServer::new(port);
        server.start(kanata_arc.clone());
        let (ntx, nrx) = crossbeam_channel::unbounded();
        (Some(server), Some(ntx), Some(nrx))
    } else {
        (None, None, None)
    };

    let (tx, rx) = crossbeam_channel::unbounded();
    Kanata::start_processing_loop(kanata_arc.clone(), rx, ntx);

    if let (Some(server), Some(nrx)) = (server, nrx) {
        Kanata::start_notification_loop(nrx, server.connections);
    }

    Kanata::event_loop(kanata_arc, tx)?;

    Ok(())
}

fn main() -> Result<()> {
    let ret = main_impl();
    if let Err(e) = ret {
        log::error!("{e}\n");
        std::process::exit(1);
    }
    eprintln!("\nPress any key to exit");
    let _ = std::io::stdin().read_line(&mut String::new());
    ret
}
