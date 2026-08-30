use std::sync::LazyLock;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

static ORIGIN: LazyLock<(Instant, f64)> = LazyLock::new(|| {
    let epoch_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
        * 1_000.0;
    (Instant::now(), epoch_ms)
});

/// Epoch-shaped timestamp backed by a monotonic clock after initialization.
pub fn monotonic_epoch_ms() -> f64 {
    ORIGIN.1 + ORIGIN.0.elapsed().as_secs_f64() * 1_000.0
}
