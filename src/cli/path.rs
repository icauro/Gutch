use std::path::PathBuf;

use crate::cli::common::write_report;
use crate::cli::path_compare::compare_paths;
use crate::cli::reporting::optional_output_paths;
use gutch as g;

pub(crate) fn compare(
    expected: PathBuf,
    actual: PathBuf,
    summary_out: Option<PathBuf>,
    stats_json_out: Option<PathBuf>,
) -> g::Result<()> {
    let stats = compare_paths(
        &expected,
        &actual,
        optional_output_paths(&summary_out, &stats_json_out),
    )?;
    write_report(&summary_out, &stats_json_out, &stats)?;

    if stats.compare_passed {
        println!(
            "binary compare passed: {} matched, {} expected files",
            stats.matched_file_count, stats.expected_file_count
        );
    } else {
        eprintln!(
            "binary compare failed: mismatched={}, missing={}, extra={}",
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
        std::process::exit(1);
    }

    Ok(())
}
