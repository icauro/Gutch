//! Unpacking the release archives: `.zip` for Windows, `.tar.xz` for Linux.

use std::fs::{self, File};
use std::io::BufReader;
use std::path::{Path, PathBuf};

use crate::publish::Result;

/// Unpacks `archive` into the empty folder `target` and returns the single
/// folder the archive holds, such as `blender-5.2.4-windows-x64`.
pub fn extract(archive: &Path, target: &Path) -> Result<PathBuf> {
    let name = archive
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let failed = |error: &dyn std::fmt::Display| format!("could not unpack {name}: {error}");
    fs::create_dir_all(target).map_err(|error| failed(&error))?;
    let file = BufReader::new(File::open(archive).map_err(|error| failed(&error))?);
    if name.ends_with(".zip") {
        zip::ZipArchive::new(file)
            .and_then(|mut zip| zip.extract(target))
            .map_err(|error| failed(&error))?;
    } else if name.ends_with(".tar.xz") {
        // Unix modes come along, so the executable flags are captured.
        tar::Archive::new(liblzma::read::XzDecoder::new(file))
            .unpack(target)
            .map_err(|error| failed(&error))?;
    } else {
        return Err(failed(&"not a .zip or .tar.xz file"));
    }
    let entries: Vec<_> = fs::read_dir(target)
        .map_err(|error| failed(&error))?
        .collect::<std::io::Result<_>>()
        .map_err(|error| failed(&error))?;
    match entries.as_slice() {
        [entry] if entry.path().is_dir() => Ok(entry.path()),
        _ => Err(failed(&"expected one top folder")),
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    #[test]
    fn a_zip_unpacks_to_its_top_folder() {
        let temp = tempfile::tempdir().unwrap();
        let archive = temp.path().join("b.zip");
        let mut zip = zip::ZipWriter::new(File::create(&archive).unwrap());
        let options = zip::write::SimpleFileOptions::default();
        zip.start_file("top/bin/tool.txt", options).unwrap();
        zip.write_all(b"hello").unwrap();
        zip.finish().unwrap();

        let top = extract(&archive, &temp.path().join("out")).unwrap();
        assert_eq!(top, temp.path().join("out").join("top"));
        assert_eq!(fs::read(top.join("bin/tool.txt")).unwrap(), b"hello");
    }

    #[test]
    fn a_tar_xz_unpacks_to_its_top_folder() {
        let temp = tempfile::tempdir().unwrap();
        let archive = temp.path().join("b.tar.xz");
        let encoder = liblzma::write::XzEncoder::new(File::create(&archive).unwrap(), 6);
        let mut tar = tar::Builder::new(encoder);
        let mut header = tar::Header::new_gnu();
        header.set_size(5);
        header.set_mode(0o755);
        header.set_cksum();
        tar.append_data(&mut header, "top/run", &b"hello"[..])
            .unwrap();
        tar.into_inner().unwrap().finish().unwrap();

        let top = extract(&archive, &temp.path().join("out")).unwrap();
        assert_eq!(fs::read(top.join("run")).unwrap(), b"hello");
    }

    #[test]
    fn other_files_are_refused() {
        let temp = tempfile::tempdir().unwrap();
        let archive = temp.path().join("b.msi");
        fs::write(&archive, b"x").unwrap();
        let error = extract(&archive, &temp.path().join("out")).unwrap_err();
        assert!(error.contains("not a .zip"), "{error}");
    }
}
