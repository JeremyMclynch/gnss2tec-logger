mod args;
mod commands;
mod shared;

use anyhow::Result;
use clap::Parser;

use args::{AppCommand, Cli};
use commands::{run_convert, run_log, run_mode};

// Top-level entrypoint: parse CLI args and dispatch to a concrete command module.
fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        AppCommand::Log(args) => run_log(args),
        AppCommand::Convert(args) => run_convert(args),
        AppCommand::Run(args) => run_mode(args),
    }
}
