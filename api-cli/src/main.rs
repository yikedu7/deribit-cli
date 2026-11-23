#![forbid(unsafe_code)]

mod application;
mod catalog;
mod policy;
mod socket;

use std::{env, process};

fn main() {
    let mut args = env::args_os().skip(1);
    let first = args.next();
    let exit = match first.as_deref().and_then(|arg| arg.to_str()) {
        Some("api") => application::run(env::args_os().skip(1), false),
        Some("testnet") => application::run(env::args_os().skip(1), true),
        Some("--help" | "-h") if args.next().is_none() => {
            let exit = deribit_cli::application::run_stdio(env::args_os());
            if exit.code() == 0 {
                println!(
                    "\nAPI catalog and WebSocket commands\n  deribit-cli api --env <mainnet|testnet> <COMMAND>\n  deribit-cli api --help\n  deribit-cli testnet --help  (compatibility command for Testnet)"
                );
            }
            exit.code()
        }
        _ => deribit_cli::application::run_stdio(env::args_os()).code(),
    };
    process::exit(exit);
}
