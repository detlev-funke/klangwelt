//! The core's clock. Desktop: `std::time`. Browser: `std::time` does not exist there
//! (wasm32-unknown-unknown) – the web core sets the page's clock
//! ([`set`]); `Instant` has the same interface.

/// Unix time in milliseconds.
#[cfg(not(target_arch = "wasm32"))]
pub fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

#[cfg(not(target_arch = "wasm32"))]
pub use std::time::Instant;

#[cfg(target_arch = "wasm32")]
mod web {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    fn zero() -> f64 {
        0.0
    }
    // Function pointer as a number, so it works without a lock.
    static WALL: AtomicUsize = AtomicUsize::new(0);
    static MONO: AtomicUsize = AtomicUsize::new(0);

    fn get(a: &AtomicUsize) -> fn() -> f64 {
        match a.load(Ordering::Relaxed) {
            0 => zero,
            // SAFETY: only set by `set` with a valid `fn() -> f64`.
            p => unsafe { std::mem::transmute::<usize, fn() -> f64>(p) },
        }
    }

    /// Sets the page's clock: `wall` = Unix time in ms (Date.now), `mono` =
    /// monotonic ms (performance.now).
    pub fn set(wall: fn() -> f64, mono: fn() -> f64) {
        WALL.store(wall as usize, Ordering::Relaxed);
        MONO.store(mono as usize, Ordering::Relaxed);
    }

    pub fn now_ms() -> u64 {
        get(&WALL)().max(0.0) as u64
    }

    /// Like `std::time::Instant` (the page's monotonic clock).
    #[derive(Clone, Copy, Debug, PartialEq, PartialOrd)]
    pub struct Instant(f64);

    impl Instant {
        pub fn now() -> Self {
            Instant(get(&MONO)())
        }
        pub fn elapsed(&self) -> Duration {
            Duration::from_secs_f64(((Self::now().0 - self.0) / 1000.0).max(0.0))
        }
    }
}

#[cfg(target_arch = "wasm32")]
pub use web::{now_ms, set, Instant};
