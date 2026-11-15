//! Atomic filesystem operations and path safety.

pub(crate) mod constraints;
pub(crate) mod directory;
mod files;
pub(crate) mod paths;

pub(crate) use files::{
    create_dir_all, from_json_bytes, from_json_str, read, read_to_string, remove_dir_all,
    remove_file, set_read_only, symlink_metadata_if_exists, write, write_atomic_with, write_json,
};
