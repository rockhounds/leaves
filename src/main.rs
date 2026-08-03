use std::sync::{Arc, Mutex};

#[macro_use]
mod diagnostics;

mod app;
mod cli;
mod colors;
mod config;
mod core;
mod error;
mod explorer;
mod forest;
mod render;
mod scanfs;
mod state;

use app::App;
use cli::{Args, init_logging};
use config::Config;
use error::Result;
use scanfs::{ScanState, ScanUI, walk_fs};

use crate::colors::ColorScheme;

#[cfg_attr(feature = "diagnostics", tracing::instrument)]
fn main() -> Result<()> {
    init_logging()?;
    error::install_handler()?;

    let mut args = Args::parse();

    if args.include_all {
        args.include_hidden = true;
        args.include_ignored = true;
        args.include_gitignored = true;
        args.include_gitexcluded = true;
    }

    let config = Config::load()?.with_env(std::env::vars());
    let scheme = ColorScheme::new(&config);

    args.path = args.path.canonicalize()?;
    diag_info!(?config, ?args, "App config");

    let scan_state = Arc::new(Mutex::new(ScanState::default()));

    let th = {
        let state = scan_state.clone();
        let args = args.clone();
        let scheme = scheme.clone();
        std::thread::spawn(move || {
            let result = walk_fs(&scheme, &args, state.clone());
            let mut state = state.lock().unwrap();
            state.done = true;
            result
        })
    };

    let quit = diag_span!(DEBUG, "Scanning")
        .in_scope(|| ratatui::run(|term| ScanUI::new(scan_state).run(term)))?;
    if quit {
        return Ok(());
    }

    let scanned = diag_span!(DEBUG, "Gathering scan results")
        .in_scope(|| th.join().map_err(|_e| error::thread_join_error()))??;

    // After initial scan, default this to 1 for on-demand expansion
    args.max_depth = 1;

    let mut app =
        diag_span!(DEBUG, "Initializing app").in_scope(|| App::new(config, scheme, args, scanned));

    ratatui::run(|terminal| app.run(terminal))
}
