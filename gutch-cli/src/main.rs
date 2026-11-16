fn main() {
    let host = gutch::api::CommandLineHost {
        bin_name: "gutch-cli",
        ..Default::default()
    };
    if let Err(error) = gutch::api::run_command_line(std::env::args_os(), &host) {
        match error {
            // The parser's message already carries its own prefix and hints.
            gutch::api::Error::Usage(_) => eprintln!("{error}"),
            _ => eprintln!("error: {error}"),
        }
        std::process::exit(error.exit_code());
    }
}
