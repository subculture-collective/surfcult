use std::path::Path;
use std::process::ExitCode;

use surfcult::config::Config;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["check-config", file] => check_config(Path::new(file)),
        _ => {
            eprintln!("usage: surfcult check-config <channels.toml>");
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
