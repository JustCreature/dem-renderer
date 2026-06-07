//! OOM-degradation seam tests.
//!
//! The runtime OOM safety net is driven by two process-global atomics that
//! `device.on_uncaptured_error` sets on a real out-of-memory and the viewer's
//! frame loop polls to step down a tier. `signal_oom_for_testing` is the
//! production test seam that mimics that store. These tests pin the contract:
//! `OOM_OBSERVED` is an edge flag the consumer clears, while `OOM_COUNT` counts
//! up-edges monotonically so two OOMs between polls are distinguishable from one.
//!
//! No GPU needed, but the atomics are shared, so both tests are `#[serial]`.

use std::sync::atomic::Ordering;

use render_gpu::{OOM_COUNT, OOM_OBSERVED, clear_oom_flag, signal_oom_for_testing};
use serial_test::serial;

#[test]
#[serial]
fn signal_sets_observed_and_increments_count() {
    clear_oom_flag();
    assert!(
        !OOM_OBSERVED.load(Ordering::SeqCst),
        "cleared flag must be false"
    );

    let before = OOM_COUNT.load(Ordering::SeqCst);
    signal_oom_for_testing();

    assert!(
        OOM_OBSERVED.load(Ordering::SeqCst),
        "signal must raise the flag"
    );
    assert_eq!(
        OOM_COUNT.load(Ordering::SeqCst),
        before + 1,
        "signal must count exactly one up-edge"
    );
}

#[test]
#[serial]
fn clear_resets_observed_but_count_is_monotonic() {
    signal_oom_for_testing();
    let count_after_signal = OOM_COUNT.load(Ordering::SeqCst);
    assert!(OOM_OBSERVED.load(Ordering::SeqCst));

    clear_oom_flag();
    assert!(
        !OOM_OBSERVED.load(Ordering::SeqCst),
        "clear must lower the observed flag"
    );
    assert_eq!(
        OOM_COUNT.load(Ordering::SeqCst),
        count_after_signal,
        "clear must leave the up-edge count untouched"
    );
}
