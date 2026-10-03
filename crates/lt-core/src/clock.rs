//! Injected stream time keeps hold/join behavior deterministic during replay.
use crate::types::StreamTime;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

pub trait Clock: Send + Sync {
    fn now(&self) -> StreamTime;
}

/// Clones share one sample counter; no wall clock or sleeping is involved.
#[derive(Clone, Debug, Default)]
pub struct ManualClock {
    samples: Arc<AtomicU64>,
}

impl ManualClock {
    pub fn new(time: StreamTime) -> Self {
        Self {
            samples: Arc::new(AtomicU64::new(time.samples())),
        }
    }

    pub fn set(&self, time: StreamTime) {
        self.samples.store(time.samples(), Ordering::SeqCst);
    }

    pub fn advance(&self, duration: StreamTime) {
        let _ = self
            .samples
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |samples| {
                Some(samples.saturating_add(duration.samples()))
            });
    }
}

impl Clock for ManualClock {
    fn now(&self) -> StreamTime {
        StreamTime::from_samples(self.samples.load(Ordering::SeqCst))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clones_share_time_and_advance_saturates() {
        let clock = ManualClock::new(StreamTime::from_millis(100));
        let observer = clock.clone();
        clock.advance(StreamTime::from_millis(32));
        assert_eq!(observer.now(), StreamTime::from_millis(132));
        observer.set(StreamTime::from_samples(u64::MAX - 1));
        clock.advance(StreamTime::from_samples(10));
        assert_eq!(observer.now(), StreamTime::from_samples(u64::MAX));
    }
}
