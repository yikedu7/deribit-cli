#![forbid(unsafe_code)]

#[path = "testnet/mod.rs"]
mod api;

use std::{env, process};

fn main() {
    let mut args = env::args_os().skip(1);
    let first = args.next();
    let exit = match first.as_deref().and_then(|arg| arg.to_str()) {
        Some("api") => api::run(env::args_os().skip(1), false),
        Some("testnet") => api::run(env::args_os().skip(1), true),
        Some("--help" | "-h") if args.next().is_none() => {
            let exit = deribit_cli::application::run_stdio(env::args_os());
            if exit.code() == 0 {
                println!("\nAPI catalog and WebSocket commands: deribit-cli api --help");
            }
            exit.code()
        }
        _ => deribit_cli::application::run_stdio(env::args_os()).code(),
    };
    process::exit(exit);
}
