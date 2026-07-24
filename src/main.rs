use std::sync::{Arc, Mutex};

use color_eyre::Result;
use tracing::{Level, instrument, span};

mod app;
mod cli;
mod colors;
mod config;
mod core;
mod explorer;
mod forest;
mod render;
mod scanfs;
mod state;

use app::App;
use cli::{Args, init_logging};
use config::Config;
use scanfs::{ScanState, ScanUI};

use crate::{colors::ColorScheme, core::Forest, forest::par_forest, scanfs::spawn_walker};

#[instrument]
fn main() -> Result<()> {
    // let file = PathBuf::from("/tmp/foofoo");
    // let db = Database::create(file)?;
    // let write_txn = db.begin_write()?;
    // {
    //     let mut table = write_txn.open_table(TABLE)?;
    //     table.insert("my_key", &123)?;
    // }
    // write_txn.commit()?;
    init_logging()?;
    color_eyre::install()?;

    use clap::Parser as _;
    let mut args = Args::parse();

    if args.include_all {
        args.include_hidden = true;
        args.include_ignored = true;
        args.include_gitignored = true;
        args.include_gitexcluded = true;
    }

    let config = Config::load()?.with_env(std::env::vars());

    args.path = args.path.canonicalize()?;
    tracing::info!(?config, ?args, "App config");

    #[cfg(feature = "db")]
    let mut app = if args.db.is_some() {
        scan_to_db(args, config)?
    } else {
        scan_to_mem(args, config)?
    };

    #[cfg(not(feature = "db"))]
    let mut app = scan_to_mem(args, config)?;

    ratatui::run(|terminal| app.run(terminal))
}

fn scan_to_mem(mut args: Args, config: Config) -> Result<App> {
    let scheme = ColorScheme::new(&config);
    let scan_state = Arc::new(Mutex::new(ScanState::default()));

    let th = {
        let state = scan_state.clone();
        let args = args.clone();
        let scheme = scheme.clone();
        std::thread::spawn(move || -> Result<Forest> {
            let root = args.path.canonicalize()?;

            let rx = spawn_walker(&scheme, &args, state.clone(), root)?;
            let forest = par_forest(&scheme, &args, &args.path, rx, None);
            let mut state = state.lock().unwrap();
            state.done = true;
            Ok(forest)
        })
    };

    let quit = span!(Level::DEBUG, "Scanning")
        .in_scope(|| ratatui::run(|term| ScanUI::new(scan_state).run(term)))?;
    if quit {
        eyre::bail!("Quitting");
    }

    let scanned = span!(Level::DEBUG, "Gathering scan results").in_scope(|| {
        th.join()
            .map_err(|_e| eyre::eyre!("Failed to join scanner thread"))
    })??;

    // After initial scan, default this to 1 for on-demand expansion
    args.max_depth = 1;

    Ok(
        span!(Level::DEBUG, "Initializing app")
            .in_scope(|| App::new(config, scheme, args, scanned)),
    )
}

#[cfg(feature = "db")]
fn scan_to_db(mut args: Args, config: Config) -> Result<App> {
    use std::{path::PathBuf, sync::mpsc};

    use redb::Database;

    use crate::core::Entry;

    let scheme = ColorScheme::new(&config);
    let scan_state = Arc::new(Mutex::new(ScanState::default()));

    let rx = spawn_walker(
        &scheme,
        &args,
        scan_state.clone(),
        args.path.canonicalize()?,
    )?;

    let (db_tx, db_rx) = mpsc::channel::<Entry>();
    let Some(db_path) = &args.db else {
        unreachable!()
    };

    let db_path = shellexpand::full(db_path)?;

    let db_path = if db_path == ":memory:" {
        // TODO: do better
        // TODO: cleanup on exit
        tempfile::NamedTempFile::with_suffix("-leaves.db")?
            .path()
            .to_path_buf()
    } else {
        PathBuf::from(db_path.as_ref())
    };

    std::fs::create_dir_all(db_path.parent().unwrap())?;

    let db_task = {
        std::thread::spawn(move || -> Result<Database> {
            let db = Database::create(db_path)?;
            let write_txn = db.begin_write()?;

            let mut table = write_txn.open_table(core::TABLE)?;
            for entry in rx {
                use std::os::unix::ffi::OsStrExt;

                let key = entry.path.as_os_str().as_bytes();
                table.insert(key, &(entry.size as u64))?;
                db_tx.send(entry)?;
            }
            drop(table);
            write_txn.commit()?;

            Ok(db)
        })
    };

    let forest_task = {
        let state = scan_state.clone();
        let args = args.clone();
        let scheme = scheme.clone();
        std::thread::spawn(move || -> Result<Forest> {
            let forest = par_forest(&scheme, &args, &args.path, db_rx, None);
            let mut state = state.lock().unwrap();
            state.done = true;
            Ok(forest)
        })
    };

    let quit = span!(Level::DEBUG, "Scanning")
        .in_scope(|| ratatui::run(|term| ScanUI::new(scan_state).run(term)))?;
    if quit {
        eyre::bail!("Quitting");
    }

    let scanned = span!(Level::DEBUG, "Gathering scan results").in_scope(|| {
        forest_task
            .join()
            .map_err(|_e| eyre::eyre!("Failed to join scanner thread"))
    })??;

    let db = db_task
        .join()
        .map_err(|_| eyre::eyre!("Failed to join database thread"))??;

    // After initial scan, default this to 1 for on-demand expansion
    args.max_depth = 1;

    Ok(span!(Level::DEBUG, "Initializing app")
        .in_scope(|| App::new(config, scheme, args, scanned))
        .with_db(db))
}
