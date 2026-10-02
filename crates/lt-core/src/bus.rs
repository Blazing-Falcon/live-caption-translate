//! Bounded broadcast with Stats eviction before critical-event overflow.
//! Critical overflow disconnects the subscriber for explicit resynchronization.
use crate::events::PipelineEvent;
use crossbeam_channel::{bounded, Receiver, RecvError, RecvTimeoutError, Sender, TryRecvError};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex, Weak},
    time::{Duration, Instant},
};

struct Subscriber {
    queue: Mutex<VecDeque<PipelineEvent>>,
    capacity: usize,
    wake: Sender<()>,
    closed: std::sync::atomic::AtomicBool,
}

#[derive(Default)]
struct BusState {
    subscribers: Mutex<Vec<Weak<Subscriber>>>,
}

#[derive(Clone, Default)]
pub struct EventBus {
    inner: Arc<BusState>,
}

pub struct EventReceiver {
    inner: Arc<Subscriber>,
    wake: Receiver<()>,
}

impl EventReceiver {
    pub fn try_recv(&self) -> Result<PipelineEvent, TryRecvError> {
        use std::sync::atomic::Ordering;
        let mut queue = self
            .inner
            .queue
            .lock()
            .map_err(|_| TryRecvError::Disconnected)?;
        if let Some(event) = queue.pop_front() {
            return Ok(event);
        }
        if self.inner.closed.load(Ordering::Relaxed) {
            Err(TryRecvError::Disconnected)
        } else {
            Err(TryRecvError::Empty)
        }
    }

    pub fn recv_timeout(&self, timeout: Duration) -> Result<PipelineEvent, RecvTimeoutError> {
        let deadline = Instant::now() + timeout;
        loop {
            match self.try_recv() {
                Ok(event) => return Ok(event),
                Err(TryRecvError::Disconnected) => return Err(RecvTimeoutError::Disconnected),
                Err(TryRecvError::Empty) => {}
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(RecvTimeoutError::Timeout);
            }
            self.wake.recv_timeout(remaining)?;
        }
    }

    pub fn recv(&self) -> Result<PipelineEvent, RecvError> {
        loop {
            match self.recv_timeout(Duration::from_secs(60)) {
                Ok(event) => return Ok(event),
                Err(RecvTimeoutError::Disconnected) => return Err(RecvError),
                Err(RecvTimeoutError::Timeout) => {}
            }
        }
    }

    pub fn try_iter(&self) -> impl Iterator<Item = PipelineEvent> + '_ {
        std::iter::from_fn(|| self.try_recv().ok())
    }
    pub fn len(&self) -> usize {
        self.inner.queue.lock().map(|q| q.len()).unwrap_or(0)
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl EventBus {
    pub fn subscribe(&self, capacity: usize) -> EventReceiver {
        let (tx, rx) = bounded(1);
        let inner = Arc::new(Subscriber {
            queue: Mutex::new(VecDeque::new()),
            capacity: capacity.max(1),
            wake: tx,
            closed: std::sync::atomic::AtomicBool::new(false),
        });
        if let Ok(mut subscribers) = self.inner.subscribers.lock() {
            subscribers.push(Arc::downgrade(&inner));
        }
        EventReceiver { inner, wake: rx }
    }

    pub fn publish(&self, event: PipelineEvent) {
        use std::sync::atomic::Ordering;
        let Ok(mut subscribers) = self.inner.subscribers.lock() else {
            return;
        };
        subscribers.retain(|weak| {
            let Some(subscriber) = weak.upgrade() else {
                return false;
            };
            if subscriber.closed.load(Ordering::Relaxed) {
                return false;
            }
            let Ok(mut queue) = subscriber.queue.lock() else {
                return false;
            };
            if queue.len() == subscriber.capacity {
                if matches!(event, PipelineEvent::Stats(_)) {
                    tracing::debug!("Dropping Stats for a slow event subscriber");
                    return true;
                }
                if let Some(index) = queue
                    .iter()
                    .position(|e| matches!(e, PipelineEvent::Stats(_)))
                {
                    queue.remove(index);
                } else {
                    subscriber.closed.store(true, Ordering::Relaxed);
                    let _ = subscriber.wake.try_send(());
                    tracing::warn!(
                        "Event subscriber overflow; disconnecting for resynchronization"
                    );
                    return false;
                }
            }
            queue.push_back(event.clone());
            let _ = subscriber.wake.try_send(());
            true
        });
    }
}

impl Drop for BusState {
    fn drop(&mut self) {
        use std::sync::atomic::Ordering;
        if let Ok(subscribers) = self.subscribers.lock() {
            for subscriber in subscribers.iter().filter_map(Weak::upgrade) {
                subscriber.closed.store(true, Ordering::Relaxed);
                let _ = subscriber.wake.try_send(());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{ListeningStateKind, PipelineStats};

    #[test]
    fn slow_subscriber_drops_stats_without_blocking_fast_subscriber() {
        let bus = EventBus::default();
        let slow = bus.subscribe(1);
        let fast = bus.subscribe(8);
        bus.publish(PipelineEvent::Stats(PipelineStats::default()));
        bus.publish(PipelineEvent::Stats(PipelineStats::default()));
        assert_eq!(slow.len(), 1);
        assert_eq!(fast.len(), 2);
        bus.publish(PipelineEvent::ListeningState {
            state: ListeningStateKind::Listening,
        });
        assert!(matches!(
            slow.recv().unwrap(),
            PipelineEvent::ListeningState { .. }
        ));
    }

    #[test]
    fn overflowing_terminal_events_disconnects_instead_of_silently_losing_order() {
        let bus = EventBus::default();
        let rx = bus.subscribe(1);
        let event = PipelineEvent::ListeningState {
            state: ListeningStateKind::Listening,
        };
        bus.publish(event.clone());
        bus.publish(event);
        assert!(rx.recv().is_ok());
        assert!(rx.recv().is_err());
    }

    #[test]
    fn last_bus_drop_disconnects_subscribers() {
        let bus = EventBus::default();
        let rx = bus.subscribe(1);
        drop(bus);
        assert!(rx.recv().is_err());
    }

    #[test]
    fn concurrent_last_bus_drops_disconnect_subscribers() {
        let bus = EventBus::default();
        let rx = bus.subscribe(1);
        let other = bus.clone();
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let other_barrier = barrier.clone();
        let worker = std::thread::spawn(move || {
            other_barrier.wait();
            drop(other);
        });
        barrier.wait();
        drop(bus);
        worker.join().unwrap();
        assert_eq!(
            rx.recv_timeout(Duration::from_millis(100)),
            Err(RecvTimeoutError::Disconnected)
        );
    }
}
