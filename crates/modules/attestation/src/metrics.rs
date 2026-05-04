//! Prometheus counters for the attestation module's call paths.
//!
//! Phase 1 of #110: minimal counter set covering the three handler
//! entry points. Wired in `handle_register_attestor_set`,
//! `handle_register_schema`, and `handle_submit_attestation`. The
//! `/metrics` endpoint that scrapes these lives in
//! `crates/rollup/src/metrics.rs`; this module is just the
//! definitions plus the lazy registry binding.
//!
//! # Why per-counter `OnceLock`
//!
//! `prometheus`'s `register_counter!` macro uses the global default
//! registry. We could call it once in module init and stash the
//! returned `Counter` in a `OnceLock`, but the SDK doesn't give us
//! a single "module loaded" hook to do that on. A lazy
//! `OnceLock<Counter>` per metric handles the cold-start case
//! cleanly: the first handler call registers, every later call
//! reads the cached `Counter`.
//!
//! Gated behind the `native` feature at the module include site
//! in `lib.rs` so the in-zkVM guest build doesn't pull `prometheus`
//! for no reason. The host build always enables `native`, so
//! `record_*` is always live there.

use std::sync::OnceLock;

use prometheus::{register_int_counter, IntCounter};

/// Counter incremented once per `RegisterAttestorSet` handler
/// invocation that returns `Ok(())`. A failed registration (e.g.
/// `ZeroThreshold`, `DuplicateAttestorSet`) does NOT increment.
/// Failure breakdown by reason lands in Phase 2 via a labelled
/// counter vector; see #110 follow-up.
fn attestor_sets_registered() -> &'static IntCounter {
    static M: OnceLock<IntCounter> = OnceLock::new();
    M.get_or_init(|| {
        register_int_counter!(
            "ligate_attestor_sets_registered_total",
            "Number of attestor sets successfully registered on-chain since the node started."
        )
        .expect("counter registers once")
    })
}

/// Counter incremented once per successful `RegisterSchema`. Same
/// shape and semantics as the attestor-set counter.
fn schemas_registered() -> &'static IntCounter {
    static M: OnceLock<IntCounter> = OnceLock::new();
    M.get_or_init(|| {
        register_int_counter!(
            "ligate_schemas_registered_total",
            "Number of schemas successfully registered on-chain since the node started."
        )
        .expect("counter registers once")
    })
}

/// Counter incremented once per successful `SubmitAttestation`.
fn attestations_submitted() -> &'static IntCounter {
    static M: OnceLock<IntCounter> = OnceLock::new();
    M.get_or_init(|| {
        register_int_counter!(
            "ligate_attestations_submitted_total",
            "Number of attestations successfully submitted on-chain since the node started."
        )
        .expect("counter registers once")
    })
}

// ----- Public API ------------------------------------------------------------

/// Bump the attestor-set-registered counter. Called after a
/// successful `RegisterAttestorSet` handler.
pub fn record_attestor_set_registered() {
    attestor_sets_registered().inc();
}

/// Bump the schema-registered counter. Called after a successful
/// `RegisterSchema` handler.
pub fn record_schema_registered() {
    schemas_registered().inc();
}

/// Bump the attestation-submitted counter. Called after a successful
/// `SubmitAttestation` handler.
pub fn record_attestation_submitted() {
    attestations_submitted().inc();
}

/// Touch every metric so they show up in the registry at startup
/// even before any handler fires. Without this, a brand-new node
/// returns an empty `/metrics` response until the first tx lands,
/// which trips alerting rules that expect a known metric set.
pub fn init() {
    let _ = attestor_sets_registered();
    let _ = schemas_registered();
    let _ = attestations_submitted();
}
