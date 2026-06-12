// On Windows, suppress the extra console window for the GUI build.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use pitstopx_lib::cli::{self, CliMode};

fn main() {
    init_tracing();

    let args: Vec<String> = std::env::args().skip(1).collect();
    match cli::parse(&args) {
        CliMode::Check => run_async(cli::check()),
        CliMode::PrintPaths => {
            if let Err(e) = cli::print_paths() {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
        CliMode::Screenshot => {
            // Demo mode: masked sample emails for store/README captures.
            std::env::set_var("PITSTOPX_DEMO", "1");
            pitstopx_lib::run();
        }
        CliMode::Tray => pitstopx_lib::run(),
    }
}

/// Run a one-shot async CLI task on a fresh Tokio runtime.
fn run_async(fut: impl std::future::Future<Output = anyhow::Result<()>>) {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    if let Err(e) = rt.block_on(fut) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn init_tracing() {
    use tracing_subscriber::{fmt, EnvFilter};
    let filter = EnvFilter::try_from_env("PITSTOPX_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = fmt().with_env_filter(filter).with_target(false).try_init();
}
