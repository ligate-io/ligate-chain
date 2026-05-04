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

use prometheus::{register_int_counter, register_int_counter_vec, IntCounter, IntCounterVec};

use crate::AttestationError;

/// Counter incremented once per `RegisterAttestorSet` handler
/// invocation that returns `Ok(())`. A failed registration (e.g.
/// `ZeroThreshold`, `DuplicateAttestorSet`) does NOT increment;
/// failures bump `attestations_rejected_total{reason}` instead.
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

/// Vector counter labelled by `reason`. Bumps once per call-handler
/// invocation that returns an [`AttestationError`]. The label value
/// comes from [`AttestationError::discriminant`] and is part of the
/// observability wire format: stable across Rust-side variant
/// renames, only changes via a coordinated dashboard update.
///
/// Per-handler kind isn't broken out (no separate counter for
/// "register-attestor-set rejects" vs "submit-attestation rejects")
/// because the failure space across handlers is mostly disjoint
/// (`zero_threshold` only fires from `RegisterAttestorSet`,
/// `unknown_schema` only from `SubmitAttestation`). Adding a `kind`
/// label later if dashboards need it is a non-breaking change.
fn attestations_rejected() -> &'static IntCounterVec {
    static M: OnceLock<IntCounterVec> = OnceLock::new();
    M.get_or_init(|| {
        register_int_counter_vec!(
            "ligate_attestations_rejected_total",
            "Number of attestation-module call handler invocations rejected with a typed AttestationError, by reason.",
            &["reason"]
        )
        .expect("counter vec registers once")
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

/// Bump the rejection counter under the variant's stable
/// discriminant string. Called from the call dispatcher when a
/// handler returns an [`AttestationError`].
pub fn record_rejected(err: &AttestationError) {
    attestations_rejected().with_label_values(&[err.discriminant()]).inc();
}

/// Touch every metric so they show up in the registry at startup
/// even before any handler fires. Without this, a brand-new node
/// returns an empty `/metrics` response until the first tx lands,
/// which trips alerting rules that expect a known metric set.
///
/// The labelled rejection vector doesn't pre-emit per-label series
/// (Prometheus aggregates 0 across unseen labels), but its `HELP`
/// and `TYPE` lines are emitted so the metric is discoverable via
/// `/metrics` even before a rejection happens.
pub fn init() {
    let _ = attestor_sets_registered();
    let _ = schemas_registered();
    let _ = attestations_submitted();
    let _ = attestations_rejected();
}
