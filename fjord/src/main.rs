//! Fjord — the NorOS compositor and window manager.

mod cursor;
mod grabs;
mod input;
mod state;
mod udev;

fn main() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stderr()))
        .init();

    tracing::info!(version = env!("CARGO_PKG_VERSION"), "fjord starting");
    if let Err(err) = udev::run() {
        tracing::error!("fjord failed: {err}");
        std::process::exit(1);
    }
}
