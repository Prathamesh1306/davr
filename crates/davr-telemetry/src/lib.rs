use davr_storage::Database;
use davr_types::{ProjectId, Result, SessionId, Severity};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tracing::debug;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelemetryEvent {
    pub kind: String,
    pub severity: Severity,
    pub ref_table: Option<String>,
    pub ref_id: Option<String>,
    pub payload: Option<serde_json::Value>,
    pub occurred_at: i64,
}

struct QueuedEvent {
    kind: String,
    severity: Severity,
    ref_table: Option<String>,
    ref_id: Option<String>,
    payload: Option<String>,
}

pub struct TelemetryEmitter {
    db: Arc<Mutex<Database>>,
    project_id: ProjectId,
    session_id: Option<SessionId>,
    enabled: bool,
    queue: Arc<Mutex<Vec<QueuedEvent>>>,
    last_flush: Arc<Mutex<Instant>>,
}

impl TelemetryEmitter {
    pub fn new(
        db: Arc<Mutex<Database>>,
        project_id: ProjectId,
        session_id: Option<SessionId>,
        enabled: bool,
    ) -> Self {
        Self {
            db,
            project_id,
            session_id,
            enabled,
            queue: Arc::new(Mutex::new(Vec::new())),
            last_flush: Arc::new(Mutex::new(Instant::now())),
        }
    }

    /// Emits a structured telemetry event into the batching queue
    pub fn emit(
        &self,
        kind: &str,
        severity: Severity,
        ref_table: Option<&str>,
        ref_id: Option<&str>,
        payload: Option<serde_json::Value>,
    ) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }

        let queued = QueuedEvent {
            kind: kind.to_string(),
            severity,
            ref_table: ref_table.map(|s| s.to_string()),
            ref_id: ref_id.map(|s| s.to_string()),
            payload: payload.map(|p| p.to_string()),
        };

        let mut queue = self.queue.lock().unwrap();
        queue.push(queued);

        let mut last_flush = self.last_flush.lock().unwrap();
        if queue.len() >= 50 || last_flush.elapsed().as_millis() >= 200 {
            self.flush_locked(&mut queue);
            *last_flush = Instant::now();
        }

        Ok(())
    }

    /// Forces a synchronous flush of all queued events to SQLite
    pub fn flush(&self) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }
        let mut queue = self.queue.lock().unwrap();
        self.flush_locked(&mut queue);
        Ok(())
    }

    fn flush_locked(&self, queue: &mut Vec<QueuedEvent>) {
        if queue.is_empty() {
            return;
        }

        let events: Vec<QueuedEvent> = std::mem::take(queue);
        let db = self.db.lock().unwrap();

        for event in events {
            let _ = db.record_telemetry_event(
                &self.project_id,
                self.session_id.as_ref(),
                &event.kind,
                event.severity,
                event.ref_table.as_deref(),
                event.ref_id.as_deref(),
                event.payload.as_deref(),
            );
        }

        debug!("Flushed telemetry events to SQLite");
    }
}
