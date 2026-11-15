//! `tree compare` command.

use std::path::PathBuf;

use crate::cli::command_support::write_report;
use crate::cli::reporting::optional_output_paths;
use crate::path_compare::compare_paths;

pub(crate) fn compare(
    expected: PathBuf,
    actual: PathBuf,
    stats_json_out: Option<PathBuf>,
) -> crate::Result<()> {
    let stats = compare_paths(&expected, &actual)?;
    #[derive(serde::Serialize)]
    struct Report<'a> {
        command: &'static str,
        #[serde(flatten)]
        result: &'a crate::path_compare::PathComparisonResult,
        outputs: crate::cli::reporting::OptionalOutputPaths,
    }
    write_report(
        &stats_json_out,
        &Report {
            command: "tree compare",
            result: &stats,
            outputs: optional_output_paths(&stats_json_out),
        },
        None,
    )?;

    if stats.compare_passed {
        crate::cli::output::line(format_args!(
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
