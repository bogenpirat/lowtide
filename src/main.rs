#![windows_subsystem = "windows"]

mod model;
mod monitor;
mod probe;
mod sources;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--probe") {
        std::process::exit(probe::run());
    }
    if let Some(i) = args.iter().position(|a| a == "--watch") {
        let secs = args.get(i + 1).and_then(|s| s.parse().ok()).unwrap_or(60);
        std::process::exit(probe::watch(secs));
    }
}
