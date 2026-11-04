#![windows_subsystem = "windows"]

mod model;
mod probe;
mod sources;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--probe") {
        std::process::exit(probe::run());
    }
}
