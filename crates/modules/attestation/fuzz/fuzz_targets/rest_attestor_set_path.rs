//! REST extractor fuzz target for `/attestor-sets/{attestor_set_id}`.
//!
//! Sibling of `rest_schema_path`. Same shape, different identifier
//! type — exercises the `las1...` decoder under the Axum extractor
//! pipeline.
//!
//! Tracking issue: ligate-io/ligate-chain#157.

#![no_main]

use std::str::FromStr;

use attestation::AttestorSetId;
use axum::extract::Path;
use axum::routing::get;
use axum::Router;
use http::Request;
use http_body_util::Empty;
use libfuzzer_sys::fuzz_target;
use once_cell::sync::Lazy;
use tower::ServiceExt;

static APP: Lazy<Router> = Lazy::new(|| {
    Router::new().route("/attestor-sets/{attestor_set_id}", get(noop_handler))
});

static RUNTIME: Lazy<tokio::runtime::Runtime> = Lazy::new(|| {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build tokio runtime")
});

async fn noop_handler(Path(_id): Path<AttestorSetId>) -> &'static str {
    "ok"
}

fuzz_target!(|data: &[u8]| {
    let Ok(s) = core::str::from_utf8(data) else {
        return;
    };
    if s.len() > 4096 {
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
    let uri = format!("/attestor-sets/{}", encoded);
    let Ok(req) = Request::builder().uri(&uri).body(Empty::<axum::body::Bytes>::new()) else {
        return;
    };

    RUNTIME.block_on(async {
        let _ = APP.clone().oneshot(req).await;
    });

    let _ = AttestorSetId::from_str(s);
});
