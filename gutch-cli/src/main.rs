fn main() {
    if let Err(error) = gutch::api::run_from_args(std::env::args_os()) {
        match error {
            // The parser's message already carries its own prefix and hints.
            gutch::api::Error::Usage(_) => eprintln!("{error}"),
            _ => eprintln!("error: {error}"),
        }
        std::process::exit(error.exit_code());
    }
}
