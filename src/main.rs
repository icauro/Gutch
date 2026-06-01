#![warn(clippy::all)]
#![warn(rust_2024_compatibility)]

mod cli;

use clap::Parser;

fn main() {
    let cli = cli::Cli::parse();
    crate::cli::set_verbose_output_enabled(cli.verbose);

    if let Err(e) = cli::commands::run(cli) {
        eprintln!("error: {}", e);
        std::process::exit(1);
    }
}
