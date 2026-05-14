mod app;
mod cli;
mod gui;
mod shell;
mod tray;

use std::io;

use clap::Parser;
use tracing_subscriber::EnvFilter;

use crate::cli::{Cli, Command};

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
    let ui_entrypoint = match &cli.command {
        None => !cli.global.json && !cli.global.dry_run && !cli.global.confirm && !cli.global.quiet,
        Some(Command::Gui(_)) | Some(Command::Tray(_)) => true,
        Some(_) => false,
    };
    let exit_code = match app::run(cli) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("guardian error: {error}");
            if ui_entrypoint {
                show_startup_error_dialog(&error.to_string());
            }
            1
        }
    };

    std::process::exit(exit_code);
}

#[cfg(windows)]
fn show_startup_error_dialog(message: &str) {
    use windows::{
        Win32::{
            Foundation::HWND,
            UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW},
        },
        core::PCWSTR,
    };

    let title = wide_null("Guardian 启动失败");
    let body = wide_null(&format!(
        "Guardian GUI 无法启动。\r\n\r\n错误：{message}\r\n\r\n请从 PowerShell 或 CMD 运行 guardian.exe gui 查看完整诊断输出。"
    ));

    unsafe {
        let _ = MessageBoxW(
            HWND(std::ptr::null_mut()),
            PCWSTR(body.as_ptr()),
            PCWSTR(title.as_ptr()),
            MB_OK | MB_ICONERROR,
        );
    }
}

#[cfg(not(windows))]
fn show_startup_error_dialog(_message: &str) {}

#[cfg(windows)]
fn wide_null(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}
