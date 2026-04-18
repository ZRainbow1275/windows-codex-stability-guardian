mod app;
mod cli;
mod gui;
mod shell;
mod tray;

use std::io;

use clap::Parser;
use tracing_subscriber::EnvFilter;

use crate::cli::Cli;

fn init_tracing() {
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_writer(io::stderr)
        .compact()
        .init();
}

fn main() {
    init_tracing();

    let cli = Cli::parse();
    let exit_code = match app::run(cli) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("guardian error: {error}");
            1
        }
    };

    std::process::exit(exit_code);
}
