//! Snapshot returned by `get_state`, kept current from the pipeline event stream.
use lt_core::events::{
    EngineKind, EngineState, ListeningStateKind, PipelineEvent, SourceInfo, SourceStateKind,
};
use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct EngineMap {
    pub asr: EngineState,
    pub translator: EngineState,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AppState {
    pub listening: ListeningStateKind,
    pub source: Option<SourceInfo>,
    pub source_state: SourceStateKind,
    pub engines: EngineMap,
    pub models_ready: bool,
    pub overlay_moving: bool,
    pub overlay_visible: bool,
    /// Logical CPUs of this PC: the UI offers up to this many threads.
    pub logical_cpus: u32,
}

impl Default for AppState {
    /// Before the first start nothing is loaded; engines read as ready so the overlay
    /// shows no loading text until a start actually reports `Loading`.
    fn default() -> Self {
        Self {
            listening: ListeningStateKind::Paused,
            source: None,
            source_state: SourceStateKind::Playing,
            engines: EngineMap {
                asr: EngineState::Ready,
                translator: EngineState::Ready,
            },
            models_ready: false,
            overlay_moving: false,
            overlay_visible: true,
            logical_cpus: lt_core::config::logical_cpus(),
        }
    }
}

impl AppState {
    /// Returns true when the snapshot changed (callers refresh the tray).
    pub fn apply(&mut self, event: &PipelineEvent) -> bool {
        let before = self.clone();
        match event {
            PipelineEvent::ListeningState { state } => self.listening = *state,
            PipelineEvent::SourceChanged { info } => self.source = Some(info.clone()),
            PipelineEvent::SourceState { state, .. } => self.source_state = *state,
            PipelineEvent::EngineStatus { engine, state, .. } => match engine {
                EngineKind::Asr => self.engines.asr = *state,
                EngineKind::Translator => self.engines.translator = *state,
            },
            _ => {}
        }
        *self != before
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_shape_matches_the_contract() {
        let json = serde_json::to_value(AppState::default()).unwrap();
        assert_eq!(json["listening"], "paused");
        assert_eq!(json["engines"]["translator"], "ready");
        assert!(json["source"].is_null());
        assert_eq!(json["models_ready"], false);
    }
}
