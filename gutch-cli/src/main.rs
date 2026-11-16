fn main() {
    if let Err(error) =
        gutch::api::run_command_line(std::env::args_os(), &gutch::api::CommandLineHost::default())
    {
        match error {
            // The parser's message already carries its own prefix and hints.
            gutch::api::Error::Usage(_) => eprintln!("{error}"),
            _ => eprintln!("error: {error}"),
        }
        std::process::exit(error.exit_code());
    }
}
