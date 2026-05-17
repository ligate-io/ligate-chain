//! REST extractor fuzz target for `/attestations/{attestation_id}`.
//!
//! Since v0.2.0, `AttestationId` is a single 32-byte bech32m id with
//! the `lat` HRP (`lat1...`); this fuzz target exercises the same
//! macro-generated `FromStr` path as `SchemaId` / `AttestorSetId` /
//! `PayloadHash`, with adversarial percent-encoded inputs in the URL.
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
        // 8KB cap: well beyond any plausible bech32 `lat1...` length
        // (~63 chars), enough headroom for percent-encoded adversarial
        // inputs the URL parser might still accept.
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
