//! Prometheus counters for the contract module's call paths.
//!
//! v0 minimal counter: one aggregate "contract call accepted" so the
//! ops dashboard sees the module's traffic from day 1. Per-variant
//! counters (post / commit / deliver / accept / reject / resolve /
//! cancel) land alongside the handler implementation PR; same shape
//! as the attestation + bounty modules.

use std::sync::OnceLock;

use prometheus::{register_int_counter, IntCounter};

fn contract_call_counter() -> &'static IntCounter {
    static CELL: OnceLock<IntCounter> = OnceLock::new();
    CELL.get_or_init(|| {
        register_int_counter!(
            "ligate_contract_calls_total",
            "Total `CallMessage` invocations accepted by the contract module."
        )
        .expect("counter registration succeeds on first call")
    })
}

/// Increment `ligate_contract_calls_total`.
pub fn record_contract_call() {
    contract_call_counter().inc();
}
