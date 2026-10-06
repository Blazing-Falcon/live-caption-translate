//! Reads the core event bus on a dedicated thread, mirrors state for `get_state`, and forwards
//! events to both windows through the delta coalescer.
use crate::{
    app::{lock, Shared},
    coalesce::Coalescer,
};
use crossbeam_channel::RecvTimeoutError;
use lt_core::events::PipelineEvent;
use std::{
    sync::{atomic::Ordering, Arc},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use tauri::Emitter;

const IDLE_WAIT: Duration = Duration::from_millis(100);
pub const EVENT: &str = "pipeline://event";

pub fn spawn(shared: Arc<Shared>) -> std::io::Result<JoinHandle<()>> {
    thread::Builder::new()
        .name("lt-bridge".into())
        .spawn(move || {
            let mut coalescer = Coalescer::default();
            let mut events = shared.bus.subscribe(4096);
            loop {
                if shared.quitting.load(Ordering::Acquire) {
                    return;
                }
                let wait = coalescer.next_deadline().map_or(IDLE_WAIT, |deadline| {
                    deadline
                        .saturating_duration_since(Instant::now())
                        .min(IDLE_WAIT)
                });
                match events.recv_timeout(wait) {
                    Ok(event) => {
                        observe(&shared, &event);
                        for out in coalescer.push(event, Instant::now()) {
                            forward(&shared, &out);
                        }
                    }
                    Err(RecvTimeoutError::Timeout) => {
                        for out in coalescer.flush_due(Instant::now()) {
                            forward(&shared, &out);
                        }
                    }
                    Err(RecvTimeoutError::Disconnected) => {
                        // The bus drops subscribers that overflow on critical events.
                        tracing::warn!("Bridge subscriber overflowed; resubscribing");
                        events = shared.bus.subscribe(4096);
                    }
                }
            }
        })
}

fn observe(shared: &Shared, event: &PipelineEvent) {
    if let PipelineEvent::Stats(stats) = event {
        *lock(&shared.stats) = stats.clone();
    }
    let changed = lock(&shared.state).apply(event);
    if changed || matches!(event, PipelineEvent::Stats(_)) {
        shared.refresh_tray();
    }
}

fn forward(shared: &Shared, event: &PipelineEvent) {
    for window in ["overlay", "control"] {
        if let Err(error) = shared.handle.emit_to(window, EVENT, event) {
            tracing::debug!(%error, window, "Event not delivered");
        }
    }
}
