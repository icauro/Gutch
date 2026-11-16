//! `debug compare-trees` command.

use std::path::PathBuf;

use crate::cli::output;
use crate::cli::reporting::write_stats_json_output;
use crate::path_compare::PathComparisonResult;
use crate::path_compare::compare_paths;

pub(crate) fn compare(expected: PathBuf, actual: PathBuf) -> crate::Result<()> {
    let stats = compare_paths(&expected, &actual)?;
    #[derive(serde::Serialize)]
    struct Report<'a> {
        command: &'static str,
        #[serde(flatten)]
        result: &'a PathComparisonResult,
    }
    write_stats_json_output(
        &Report {
            command: "tree compare",
            result: &stats,
        },
        None,
    )?;

    if stats.compare_passed {
        output::line(format_args!(
            "tree comparison passed: {} matched, {} expected files",
            stats.matched_file_count, stats.expected_file_count
        ));
    } else {
        eprintln!(
            "tree comparison failed: mismatched={}, missing={}, extra={}",
            stats.mismatched_file_count, stats.missing_file_count, stats.extra_file_count
        );
        for relative_path in &stats.mismatched_files {
            eprintln!("  MISMATCH: {}", relative_path);
        }
        for relative_path in &stats.missing_files {
            eprintln!("  MISSING: {}", relative_path);
        }
        for relative_path in &stats.extra_files {
            eprintln!("  EXTRA: {}", relative_path);
        }
        return Err(crate::Error::verify("tree comparison failed"));
    }

    Ok(())
}
