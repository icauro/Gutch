use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FetchProgressEvent {
    pub phase: String,
    pub message: String,
    pub current_bundle: usize,
    pub total_bundles: usize,
    pub transferred_bytes: u64,
    pub total_bytes: u64,
    pub done: bool,
    pub failed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub trait FetchProgressSink: Send + Sync {
    fn emit(&self, event: FetchProgressEvent);
}

pub(crate) fn emit_progress(
    progress: &Option<std::sync::Arc<dyn FetchProgressSink>>,
    event: FetchProgressEvent,
) {
    if let Some(sink) = progress {
        sink.emit(event);
    }
}
