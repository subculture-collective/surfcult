use std::path::Path;
use std::process::ExitCode;

use surfcult::config::Config;
use surfcult::server::{self, ServerSettings};
use surfcult::store::Store;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["check-config", file] => check_config(Path::new(file)),
        ["serve", config, db] => serve(Path::new(config), Path::new(db)),
        _ => {
            eprintln!(
                "usage: surfcult check-config <channels.toml>\n       surfcult serve <channels.toml> <db>"
            );
            ExitCode::from(2)
        }
    }
}

fn check_config(path: &Path) -> ExitCode {
    match Config::load(path) {
        Ok(config) => {
            println!(
                "{}: ok ({} channels, {} pools)",
                path.display(),
                config.channels.len(),
                config.pools.len()
            );
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("{}: invalid\n{err}", path.display());
            ExitCode::FAILURE
        }
    }
}

/// Serves the read API over `config_path` and the database at `db_path`
/// until the listener fails. See `docs/reference/api.md`.
fn serve(config_path: &Path, db_path: &Path) -> ExitCode {
    let config = match Config::load(config_path) {
        Ok(config) => config,
        Err(err) => {
            eprintln!("{}: invalid\n{err}", config_path.display());
            return ExitCode::FAILURE;
        }
    };
    let result = ServerSettings::from_env()
        .map_err(|err| err.to_string())
        .and_then(|settings| {
            let store = Store::open_read_only(db_path)
                .map_err(|err| format!("{}: {err}", db_path.display()))?;
            // Axum's accept loop sleeps after an accept error, so the timer
            // must be on as well as IO.
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|err| err.to_string())?;
            runtime.block_on(async {
                let listener = tokio::net::TcpListener::bind(settings.addr)
                    .await
                    .map_err(|err| format!("{}: {err}", settings.addr))?;
                let addr = listener.local_addr().map_err(|err| err.to_string())?;
                eprintln!("serving the read API on http://{addr}");
                server::serve(listener, settings.token, config, store)
                    .await
                    .map_err(|err| err.to_string())
            })
        });
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("surfcult serve: {err}");
            ExitCode::FAILURE
        }
    }
}
