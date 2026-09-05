fn main() {
    if let Err(error) = gutch::command_api::run_from_args(std::env::args_os()) {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}
