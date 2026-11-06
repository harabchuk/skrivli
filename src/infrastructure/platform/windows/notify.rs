//! Completion notices for the tray icon.
//!
//! `transcript_ready` only enqueues a recording id. The tray thread drains that
//! queue and calls `Shell_NotifyIconW`. This module does not call Win32.

use std::sync::mpsc::Sender;
use std::sync::Arc;

use crate::domain::ids::RecordingId;
use crate::services::ports::TranscriptNotifier;

pub(crate) struct QueuedNotifier {
    tx: Sender<RecordingId>,
}

impl TranscriptNotifier for QueuedNotifier {
    fn transcript_ready(&self, recording_id: RecordingId) {
        let _ = self.tx.send(recording_id);
    }
}

pub(crate) fn completion_notifier(tx: Sender<RecordingId>) -> Arc<dyn TranscriptNotifier> {
    Arc::new(QueuedNotifier { tx })
}
