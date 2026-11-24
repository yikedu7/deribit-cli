use std::{env, process};

fn main() {
    process::exit(deribit_cli::application::run_cli(env::args_os()));
}
