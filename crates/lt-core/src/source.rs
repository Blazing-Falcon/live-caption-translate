//! Prepared-frame source seam. Native callbacks use an lt-audio SPSC ring;
//! the audio adapter downmixes/resamples before sending these 16 kHz frames.
use crate::{
    bus::EventBus,
    clock::Clock,
    error::{Error, Result},
    events::PipelineEvent,
    segment::FrameFlags,
    types::{SourceInfo, StreamTime},
};
use crossbeam_channel::{Receiver, Sender};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc,
};
use std::time::{Duration, Instant};

#[derive(Clone)]
pub struct AudioFrame {
    pub t0: StreamTime,
    pub samples: [f32; 512],
    pub flags: FrameFlags,
}

/// This is called by the prepared-frame adapter, never by the native callback.
#[derive(Clone)]
pub struct AudioProducer {
    sender: Sender<AudioFrame>,
    pub cancelled: Arc<AtomicBool>,
    pub base_time: StreamTime,
}

impl AudioProducer {
    pub fn new(
        sender: Sender<AudioFrame>,
        cancelled: Arc<AtomicBool>,
        base_time: StreamTime,
    ) -> Self {
        Self {
            sender,
            cancelled,
            base_time,
        }
    }
    pub fn send(&self, frame: AudioFrame) -> Result<()> {
        let mut frame = frame;
        loop {
            if self.cancelled.load(Ordering::Relaxed) {
                return Err(Error::Stopped);
            }
            match self.sender.send_timeout(frame, Duration::from_millis(20)) {
                Ok(()) => return Ok(()),
                Err(crossbeam_channel::SendTimeoutError::Timeout(returned)) => frame = returned,
                Err(crossbeam_channel::SendTimeoutError::Disconnected(_)) => {
                    return Err(Error::Stopped)
                }
            }
        }
    }
}

#[derive(Clone)]
pub struct SourceEvents {
    bus: EventBus,
}

impl SourceEvents {
    pub fn new(bus: EventBus) -> Self {
        Self { bus }
    }
    pub fn publish(&self, event: PipelineEvent) {
        self.bus.publish(event);
    }
}

pub trait AudioSource: Send {
    fn start(&mut self, output: AudioProducer, events: SourceEvents) -> Result<SourceInfo>;
    /// Stop must close the producer and join the adapter/capture thread.
    fn stop(&mut self);
}

pub type AudioReceiver = Receiver<AudioFrame>;

/// Stream positions drive fast replay; wall time keeps hold deadlines moving
/// during silence, slow engines and source interruptions.
pub struct SessionClock {
    started: Instant,
    stream_samples: AtomicU64,
}

impl Default for SessionClock {
    fn default() -> Self {
        Self {
            started: Instant::now(),
            stream_samples: AtomicU64::new(0),
        }
    }
}

impl SessionClock {
    pub fn advance_to(&self, at: StreamTime) {
        self.stream_samples
            .fetch_max(at.samples(), Ordering::Relaxed);
    }
    pub fn stream_time(&self) -> StreamTime {
        StreamTime(self.stream_samples.load(Ordering::Relaxed))
    }
}

impl Clock for SessionClock {
    fn now(&self) -> StreamTime {
        let wall = StreamTime::from_seconds(self.started.elapsed().as_secs_f64());
        StreamTime(
            wall.samples()
                .max(self.stream_samples.load(Ordering::Relaxed)),
        )
    }
}
