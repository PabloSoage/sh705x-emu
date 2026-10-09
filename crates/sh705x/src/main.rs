//! `sh705x`: command-line front end of the SH705x ECU emulator.

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = sh705x::parse(&args).and_then(|cmd| sh705x::run(&cmd));
    match result {
        Ok(text) => {
            println!("{text}");
            ExitCode::SUCCESS
        }
        Err(text) => {
            eprintln!("{text}");
            ExitCode::FAILURE
        }
    }
}
