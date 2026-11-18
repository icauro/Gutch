/// Written by build.rs: the Zstandard-compressed licenses of everything
/// compiled into this program.
const NOTICES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/notices.txt.zst"));

fn licenses() -> std::io::Result<String> {
    String::from_utf8(zstd::decode_all(NOTICES)?).map_err(std::io::Error::other)
}

fn main() {
    let host = gutch::api::CommandLineHost {
        bin_name: "gutch-cli",
        licenses: Some(&licenses),
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
