//! Embeds the license notices that `gutch-cli licenses` prints.
//!
//! Release builds set `GUTCH_CLI_NOTICES` to the file that
//! `_tools/package_cli.py notices` writes for the target: Gutch's own
//! licenses, every linked dependency's notices and the Rust standard library's.
//! Other builds embed this crate's license and say where the rest live. The
//! text is stored compressed: the standard library's notices alone are over a
//! megabyte, mostly repeated license texts.

use std::env;
use std::fs;
use std::path::PathBuf;

const SOURCE_BUILD_NOTE: &str = "\n\
This copy was built from source, so it does not bundle the notices of the
libraries compiled into it. Each library keeps its license in its own source
package. Official release executables embed all of them; see
https://github.com/icauro/Gutch/releases.
";

fn main() {
    println!("cargo:rerun-if-env-changed=GUTCH_CLI_NOTICES");
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo sets OUT_DIR"));
    let notices = match env::var_os("GUTCH_CLI_NOTICES") {
        Some(path) => {
            let path = PathBuf::from(path);
            println!("cargo:rerun-if-changed={}", path.display());
            fs::read_to_string(&path).unwrap_or_else(|error| {
                panic!(
                    "GUTCH_CLI_NOTICES names {}, which cannot be read: {error}",
                    path.display()
                )
            })
        }
        None => {
            println!("cargo:rerun-if-changed=LICENSE");
            let license = fs::read_to_string("LICENSE").expect("gutch-cli/LICENSE is readable");
            format!("{license}{SOURCE_BUILD_NOTE}")
        }
    };
    let compressed = zstd::encode_all(notices.as_bytes(), 19).expect("notices compress");
    fs::write(out.join("notices.txt.zst"), compressed).expect("OUT_DIR is writable");
}
