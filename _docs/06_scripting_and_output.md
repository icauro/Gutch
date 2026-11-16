# Scripting and output

Every command accepts the options below. They can appear before or after the
command name.

## Output formats

| Option | Output on stdout |
| --- | --- |
| *(none)* | Readable text |
| `--json` | One JSON document when the command finishes |
| `--json-lines` | One JSON object per line as events happen, ending with a line whose `kind` is `summary` |
| `--output FILE` | The selected format, written to FILE instead of stdout |
| `-v`, `--verbose` | More detail in readable output |

Progress, prompts and readable error messages go to stderr, so stdout holds
only the result. For example, `repo verify --json` prints:

```json
{
  "mode": "fast",
  "ok": true,
  "output_version": "1.0.0",
  "snapshotCount": 2,
  "snapshots": [ { "snapshotId": "74aaf4df...0bbd", "ok": true, "missingArtifactCount": 0, "...": "..." } ],
  "snapshotsOk": 2
}
```

Every JSON document, every JSON line and every error report carries
`output_version`. Fields may be added in a minor version; a field is only
removed or changed in a new major version. Check the major version before
relying on a field.

## Confirmations

Commands that change or delete data show their plan and ask before acting:
capture, snapshot delete, tag replacement, repair, push, pull and
corruption tests. In scripts:

- `--dry-run` prints the plan and changes nothing;
- `--yes` answers the question in advance.

With `--json` or `--json-lines` there is no place for a question, so these
commands fail unless `--yes` is given. Review the plan with `--dry-run` first.
`repo init` asks only for a folder that is not empty, and takes `--force`
instead of `--yes`. `--dry-run` is also accepted by `repo init`, `repo rename`,
the constraint commands, `repo index rebuild` and `installation reconstruct`.
Reconstruction does not ask before replacing its destination.

## Exit status

| Status | Meaning |
| --- | --- |
| 0 | Success |
| 1 | The operation failed or found problems, such as a failed verification, or a confirmation was declined |
| 2 | The command line was invalid, such as an unknown option or a missing argument |

## Error reports

`--error-output FILE` writes a JSON report when a command fails:

```json
{
  "code": "operation_failed",
  "message": "Verification error: 3 verification errors in /home/me/installed-app",
  "output_version": "1.0.0"
}
```

Some failures carry a more specific `code` and a `details` object:

| Code | Details |
| --- | --- |
| `filename_constraints` | Every path that breaks the repository's filename rules, with the rule and target |
| `directory_promotion_failed` | What happened when swapping a reconstructed tree into place: the step, the system error, the state of the staging, destination and backup folders, and whether rollback succeeded |
| `unsupported_format` | A repository or document written by a newer Gutch: what was found, what is supported, and whether a newer Gutch is needed |

With `--json`, the same report appears as `error_details` in the result.

## Speed and memory

Commands that read or write many files accept `--jobs N` to limit worker
threads and `--memory-budget-mib MIB` to limit working memory. By default
they use every core and a quarter of usable memory.

## Profiling

To see where time goes, add `--profile summary` for a short timing summary,
or `--profile detailed --profile-dir FOLDER` for a timeline you can open in
[Perfetto](https://ui.perfetto.dev). Profiles are diagnostic: their format
may change at any time. Reconstruction and repair show live write speed with
or without profiling.
