use crate::cli::common::write_report;
use crate::cli::path_compare::compare_paths;
use crate::cli::reporting::optional_output_paths;
use crate::{AuthCommands, BundleCommands, Cli, Commands, PathCommands};
use gutch as g;

pub(crate) fn run(cli: Cli) -> g::Result<()> {
    match cli.command {
        Commands::Auth {
            command: AuthCommands::Login { endpoint, no_open },
        } => {
            let result = crate::auth::login(&endpoint, !no_open).map_err(g::Error::manifest)?;
            if let Some(username) = result.username.as_deref() {
                println!("authenticated as {}", username);
            } else if let Some(email) = result.email.as_deref() {
                println!("authenticated as {}", email);
            } else {
                println!("authentication complete");
            }
            Ok(())
        }

        Commands::Auth {
            command:
                AuthCommands::Logout {
                    endpoint,
                    session_id,
                },
        } => {
            crate::auth::logout(&endpoint, session_id.as_deref()).map_err(g::Error::manifest)?;
            println!("logged out");
            Ok(())
        }

        Commands::Auth {
            command:
                AuthCommands::Status {
                    endpoint,
                    session_id,
                },
        } => {
            match crate::auth::session_status(&endpoint, session_id.as_deref())
                .map_err(g::Error::manifest)?
            {
                Some(status) => println!(
                    "authenticated user {} via {} (expires {})",
                    status.user_id, status.login_provider, status.expires_at
                ),
                None => println!("not authenticated"),
            }
            Ok(())
        }

        Commands::Repo { command } => crate::cli::repo::run(command),
        Commands::Target { command } => crate::cli::target::run(command),
        Commands::TargetInstall { command } => crate::cli::target_install::run(command),

        Commands::Bundle {
            command: BundleCommands::Inspect { bundle },
        } => {
            let data = std::fs::read(&bundle)?;
            let reader = g::bundle::read_bundle(&data)?;

            println!("bundle: {}", bundle.display());
            println!("  chunks: {}", reader.chunk_count());
            for entry in reader.entries() {
                println!(
                    "  - {} raw={} compressed={} ratio={:.1}%",
                    &entry.chunk_hash[..16.min(entry.chunk_hash.len())],
                    entry.raw_size,
                    entry.compressed_size,
                    100.0 * entry.compressed_size as f64 / entry.raw_size as f64
                );
            }
            Ok(())
        }

        Commands::Path {
            command:
                PathCommands::Compare {
                    expected,
                    actual,
                    summary_out,
                    stats_json_out,
                },
        } => {
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
    }
}
