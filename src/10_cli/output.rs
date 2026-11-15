//! Invocation-local output. Human messages never mix with structured stdout.
use std::cell::RefCell;
use std::fmt::Arguments;
use std::fs::File;
use std::io;
use std::io::Write;
use std::mem;

#[derive(Default)]
struct OutputState {
    json: bool,
    json_lines: bool,
    dry_run: bool,
    writer: Option<Box<dyn Write>>,
    result: Option<serde_json::Value>,
    messages: Vec<String>,
    events: Vec<serde_json::Value>,
    error: Option<io::Error>,
}

thread_local! {
    static OUTPUT: RefCell<OutputState> = RefCell::new(OutputState::default());
}

pub(crate) fn begin(cli: &super::Cli) -> crate::Result<()> {
    let writer: Box<dyn Write> = if let Some(path) = &cli.output {
        super::reporting::ensure_parent_directory(path)?;
        Box::new(File::create(path)?)
    } else {
        Box::new(io::stdout())
    };
    OUTPUT.with(|state| {
        *state.borrow_mut() = OutputState {
            json: cli.json,
            json_lines: cli.json_lines,
            dry_run: cli.dry_run,
            writer: Some(writer),
            ..Default::default()
        }
    });
    Ok(())
}

/// True when stdout carries `--json` or `--json-lines` output, which holds
/// human messages back until the command finishes.
pub(crate) fn structured() -> bool {
    OUTPUT.with(|state| {
        let state = state.borrow();
        state.json || state.json_lines
    })
}

pub(crate) fn dry_run() -> bool {
    OUTPUT.with(|state| state.borrow().dry_run)
}

pub(crate) fn line(arguments: Arguments<'_>) {
    OUTPUT.with(|state| {
        let mut state = state.borrow_mut();
        if state.json || state.json_lines {
            if state.result.is_none() {
                state.messages.push(arguments.to_string());
            }
        } else if let Some(writer) = &mut state.writer
            && let Err(error) = writeln!(writer, "{arguments}")
        {
            state.error = Some(error);
        }
    });
}

pub(crate) fn record(value: &impl serde::Serialize) -> crate::Result<()> {
    let value = serde_json::to_value(value)?;
    OUTPUT.with(|state| state.borrow_mut().result = Some(value));
    Ok(())
}

pub(crate) fn event(value: &impl serde::Serialize) -> crate::Result<()> {
    OUTPUT.with(|state| {
        let mut state = state.borrow_mut();
        if state.json_lines {
            if let Some(writer) = &mut state.writer {
                serde_json::to_writer(&mut *writer, value)?;
                writeln!(writer)?;
            }
        } else {
            let value = serde_json::to_value(value)?;
            if state.json {
                state.events.push(value);
            } else if let Some(writer) = &mut state.writer {
                let action = value
                    .get("action")
                    .and_then(|v| v.as_str())
                    .unwrap_or("artifact");
                let path = value
                    .get("path")
                    .or_else(|| value.get("id"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                writeln!(writer, "{action:<16} {path:?}")?;
            }
        }
        Ok(())
    })
}

pub(crate) fn finish(result: &crate::Result<()>) -> crate::Result<()> {
    OUTPUT.with(|state| {
        let mut state = mem::take(&mut *state.borrow_mut());
        if let Some(error) = state.error {
            return Err(error.into());
        }
        if state.json || state.json_lines {
            let mut value = state
                .result
                .unwrap_or_else(|| serde_json::json!({"ok": true, "messages": state.messages}));
            if let Err(error) = result {
                if !value.is_object() {
                    value = serde_json::json!({"result": value});
                }
                value["ok"] = serde_json::json!(false);
                value["error"] = serde_json::json!(error.to_string());
                value["error_details"] = error.diagnostic_report();
            } else if let Some(object) = value.as_object_mut() {
                // Some results carry their own `ok` (a verification verdict).
                object.entry("ok").or_insert(serde_json::Value::Bool(true));
            }
            if !state.events.is_empty() {
                value = serde_json::json!({"summary": value, "events": state.events});
            }
            if let Some(writer) = &mut state.writer {
                if state.json_lines {
                    serde_json::to_writer(
                        &mut *writer,
                        &serde_json::json!({"kind": "summary", "summary": value}),
                    )?;
                } else {
                    serde_json::to_writer_pretty(&mut *writer, &value)?;
                }
                writeln!(writer)?;
            }
        }
        if let Some(writer) = &mut state.writer {
            writer.flush()?;
        }
        Ok(())
    })
}
