use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

/// A stop flag shared between pipeline threads.
#[derive(Clone, Default)]
pub(crate) struct Cancel(Arc<AtomicBool>);

impl Cancel {
    pub(crate) fn is_set(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }

    pub(crate) fn set(&self) {
        self.0.store(true, Ordering::Release);
    }

    /// The raw flag for engine controls, which take it directly.
    pub(crate) fn flag(&self) -> Arc<AtomicBool> {
        self.0.clone()
    }
}
