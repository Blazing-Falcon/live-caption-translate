//! Splits independent row work across scoped threads.

use std::{cell::Cell, thread};
/// Work (rows times work per row) below this runs on the calling thread, where spawning costs more
/// than it saves.
const THRESHOLD: usize = 1 << 17;
thread_local! {static THREADS:Cell<Option<usize>>=const{Cell::new(None)};}
pub fn with_threads<T>(threads: usize, body: impl FnOnce() -> T) -> T {
    struct Restore(Option<usize>);
    impl Drop for Restore {
        fn drop(&mut self) {
            THREADS.set(self.0);
        }
    }
    let _restore = Restore(THREADS.replace(Some(threads.max(1))));
    body()
}
fn threads() -> usize {
    THREADS
        .get()
        .unwrap_or_else(|| thread::available_parallelism().map_or(1, usize::from))
}
pub fn for_each(rows: usize, work_per_row: usize, body: impl Fn(usize) + Sync) {
    let threads = threads();
    if rows < 2 || threads <= 1 || rows.saturating_mul(work_per_row) < THRESHOLD {
        for y in 0..rows {
            body(y);
        }
        return;
    }
    let blocks = rows.min(threads * 2);
    let workers = threads.min(blocks);
    thread::scope(|scope| {
        let body = &body;
        for worker in 0..workers {
            scope.spawn(move || {
                for block in (worker..blocks).step_by(workers) {
                    let start = rows * block / blocks;
                    let end = rows * (block + 1) / blocks;
                    for y in start..end {
                        body(y);
                    }
                }
            });
        }
    });
}
pub fn blocks(rows: usize, work_per_row: usize, body: impl Fn(usize, usize) + Sync) {
    let threads = threads();
    if rows < 4 || threads <= 1 || rows.saturating_mul(work_per_row) < THRESHOLD {
        body(0, rows);
        return;
    }
    let blocks = (rows / 2).min(threads);
    thread::scope(|scope| {
        let body = &body;
        for block in 0..blocks {
            scope.spawn(move || body(rows * block / blocks, rows * (block + 1) / blocks));
        }
    });
}

pub fn slice_rows<T: Send>(
    data: &mut [T],
    rows: usize,
    width: usize,
    work_per_row: usize,
    body: impl Fn(usize, &mut [T]) + Sync,
) {
    assert_eq!(data.len(), rows * width);
    if width == 0 {
        return;
    }
    let threads = threads();
    if rows < 2 || threads <= 1 || rows.saturating_mul(work_per_row) < THRESHOLD {
        for (y, row) in data.chunks_mut(width).enumerate() {
            body(y, row);
        }
        return;
    }
    let blocks = rows.min(threads * 2);
    let workers = threads.min(blocks);
    thread::scope(|scope| {
        let body = &body;
        let mut tail = data;
        for worker in 0..workers {
            let first_block = blocks * worker / workers;
            let last_block = blocks * (worker + 1) / workers;
            let start = rows * first_block / blocks;
            let end = rows * last_block / blocks;
            let (chunk, rest) = tail.split_at_mut((end - start) * width);
            tail = rest;
            scope.spawn(move || {
                for (i, row) in chunk.chunks_mut(width).enumerate() {
                    body(start + i, row);
                }
            });
        }
    });
}
