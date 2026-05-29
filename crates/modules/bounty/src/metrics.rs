//! Prometheus counters for the bounty module's call paths.
//!
//! v0 counter set: post / claim / paid-out. Wired into the no-op
//! handler stubs so the metric surface exists from day 1; the values
//! stay at zero until the real handlers land in follow-up PRs.
//!
//! Per-counter `OnceLock` pattern matches the attestation module's
//! `metrics.rs`. See that file for the rationale (no single
//! module-init hook in the SDK, so each metric registers lazily on
//! the first `record_*` call and caches the registered handle).
//!
//! Gated behind the `native` feature at the module include site so
//! the in-zkVM guest build doesn't pull `prometheus` for no reason.

use std::sync::OnceLock;

use prometheus::{register_int_counter, IntCounter};

fn post_bounty_counter() -> &'static IntCounter {
    static CELL: OnceLock<IntCounter> = OnceLock::new();
    CELL.get_or_init(|| {
        register_int_counter!(
            "ligate_bounty_posted_total",
            "Total `PostBounty` calls accepted by the bounty module."
        )
        .expect("counter registration succeeds on first call")
    })
}

fn claim_bounty_counter() -> &'static IntCounter {
    static CELL: OnceLock<IntCounter> = OnceLock::new();
    CELL.get_or_init(|| {
        register_int_counter!(
            "ligate_bounty_claimed_total",
            "Total `ClaimBounty` calls accepted by the bounty module."
        )
        .expect("counter registration succeeds on first call")
    })
}

#[allow(dead_code)]
fn paid_out_counter() -> &'static IntCounter {
    static CELL: OnceLock<IntCounter> = OnceLock::new();
    CELL.get_or_init(|| {
        register_int_counter!(
            "ligate_bounty_avow_paid_out_total",
            "Cumulative `AVOW` nanos paid out from bounty escrows."
        )
        .expect("counter registration succeeds on first call")
    })
}

fn finalise_bounty_counter() -> &'static IntCounter {
    static CELL: OnceLock<IntCounter> = OnceLock::new();
    CELL.get_or_init(|| {
        register_int_counter!(
            "ligate_bounty_finalised_total",
            "Total `FinaliseBounty` calls accepted by the bounty module."
        )
        .expect("counter registration succeeds on first call")
    })
}

/// Increment `ligate_bounty_posted_total`.
pub fn record_post_bounty() {
    post_bounty_counter().inc();
}

/// Increment `ligate_bounty_claimed_total`.
pub fn record_claim_bounty() {
    claim_bounty_counter().inc();
}

/// Increment `ligate_bounty_finalised_total`.
pub fn record_finalise_bounty() {
    finalise_bounty_counter().inc();
}

/// Add to `ligate_bounty_avow_paid_out_total`. Reserved for the
/// claim-handler PR; v0 doesn't call it yet.
#[allow(dead_code)]
pub fn record_avow_paid_out(amount_nano: u64) {
    paid_out_counter().inc_by(amount_nano);
}
