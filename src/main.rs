use std::{env, io, io::IsTerminal, process};

use deribit_cli::application::{ProcessExit, local_io_failure, run_process};

fn main() {
    let stdin = io::stdin();
    let stdin_is_terminal = stdin.is_terminal();
    let output = run_process(env::args_os(), &mut stdin.lock(), stdin_is_terminal);
    let mut stdout = io::stdout().lock();
    let mut stderr = io::stderr().lock();
    let exit = match output.write_to(&mut stdout, &mut stderr) {
        Ok(()) => output.exit(),
        Err(error) => {
            let fallback = local_io_failure(&error);
            let _ = fallback.write_to(&mut stdout, &mut stderr);
            ProcessExit::LocalIo
        }
    };
    process::exit(exit.code());
}
