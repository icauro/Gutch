use std::path::PathBuf;

use crate::api::compare_paths;
use crate::command_api::common::write_report;
use crate::command_api::reporting::optional_output_paths;
use gutch as g;

pub(crate) fn compare(
    expected: PathBuf,
    actual: PathBuf,
    stats_json_out: Option<PathBuf>,
) -> g::Result<()> {
    let stats = compare_paths(&expected, &actual)?;
    #[derive(serde::Serialize)]
    struct Report<'a> {
        command: &'static str,
        #[serde(flatten)]
        result: &'a g::api::PathComparisonResult,
        outputs: crate::command_api::reporting::OptionalOutputPaths,
    }
    write_report(
        &stats_json_out,
        &Report {
            command: "path compare",
            result: &stats,
            outputs: optional_output_paths(&stats_json_out),
        },
    )?;

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
        return Err(g::Error::verify("path comparison failed"));
    }

    Ok(())
}
