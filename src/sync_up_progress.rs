use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncUpProgressEvent {
    pub phase: String,
    pub message: String,
    pub current: usize,
    pub total: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    pub transferred_bytes: u64,
    pub total_bytes: u64,
    pub done: bool,
}

pub trait SyncUpProgressSink: Send + Sync {
    fn emit(&self, event: SyncUpProgressEvent);
}

pub(crate) fn emit_progress(
    progress: &Option<std::sync::Arc<dyn SyncUpProgressSink>>,
    event: SyncUpProgressEvent,
) {
    if let Some(sink) = progress {
        sink.emit(event);
    }
}
