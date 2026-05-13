//! REST extractor fuzz target for `/attestations/{attestation_id}`.
//!
//! `AttestationId` is the compound `<schema_id>:<payload_hash>` form,
//! so the path extractor exercises both the colon-delimited parse
//! AND the underlying two-identifier decoders. Strictly more surface
//! than the `rest_schema_path` and `rest_attestor_set_path` siblings.
//!
//! Tracking issue: ligate-io/ligate-chain#157.

#![no_main]

use std::str::FromStr;

use attestation::AttestationId;
use axum::extract::Path;
use axum::routing::get;
use axum::Router;
use http::Request;
use http_body_util::Empty;
use libfuzzer_sys::fuzz_target;
use once_cell::sync::Lazy;
use tower::ServiceExt;

static APP: Lazy<Router> = Lazy::new(|| {
    Router::new().route("/attestations/{attestation_id}", get(noop_handler))
});

static RUNTIME: Lazy<tokio::runtime::Runtime> = Lazy::new(|| {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build tokio runtime")
});

async fn noop_handler(Path(_id): Path<AttestationId>) -> &'static str {
    "ok"
}

fuzz_target!(|data: &[u8]| {
    let Ok(s) = core::str::from_utf8(data) else {
        return;
    };
    if s.len() > 8192 {
        // `AttestationId` is roughly 2x the size of a single id
        // (schema + payload), so the cap doubles vs the single-id
        // siblings to give the fuzzer headroom for "long but valid".
        return;
    }
    let encoded: String = s
        .as_bytes()
        .iter()
        .map(|b| {
            if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
                String::from(*b as char)
            } else {
                format!("%{:02X}", b)
            }
        })
        .collect();
    let uri = format!("/attestations/{}", encoded);
    let Ok(req) = Request::builder().uri(&uri).body(Empty::<axum::body::Bytes>::new()) else {
        return;
    };

    RUNTIME.block_on(async {
        let _ = APP.clone().oneshot(req).await;
    });

    let _ = AttestationId::from_str(s);
});
