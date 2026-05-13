//! REST extractor fuzz target for `/schemas/{schema_id}`.
//!
//! Exercises the full Axum routing pipeline + Path extractor, not
//! just the underlying `SchemaId::FromStr` decoder. Catches a class
//! of bugs that the lower-level `bech32_id` fuzz target doesn't:
//!
//! - URL percent-encoding edge cases (axum decodes before extractor)
//! - Oversized URL segments (axum rejects above a threshold)
//! - Routing-layer panics (route matching against weird inputs)
//! - Extractor rejection paths producing well-formed responses
//!
//! The handler is a no-op that takes the extracted `SchemaId` and
//! returns 200. Any input that causes a panic anywhere in the
//! routing + extracting pipeline fails the fuzz target.
//!
//! Tracking issue: ligate-io/ligate-chain#157.

#![no_main]

use std::str::FromStr;

use attestation::SchemaId;
use axum::extract::Path;
use axum::routing::get;
use axum::Router;
use http::Request;
use http_body_util::Empty;
use libfuzzer_sys::fuzz_target;
use once_cell::sync::Lazy;
use tower::ServiceExt;

/// Build the Axum router once per fuzz binary. Each fuzz iteration
/// then clones-and-routes against this shared instance — much faster
/// than reconstructing per-input.
static APP: Lazy<Router> = Lazy::new(|| {
    Router::new().route("/schemas/{schema_id}", get(noop_handler))
});

/// Tokio runtime shared across fuzz iterations. libfuzzer-sys's
/// `fuzz_target!` is sync; `block_on` bridges to async.
static RUNTIME: Lazy<tokio::runtime::Runtime> = Lazy::new(|| {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build tokio runtime")
});

async fn noop_handler(Path(_schema_id): Path<SchemaId>) -> &'static str {
    "ok"
}

fuzz_target!(|data: &[u8]| {
    // The path segment must be valid UTF-8 to be a URL at all.
    let Ok(s) = core::str::from_utf8(data) else {
        return;
    };
    // Reject huge inputs to keep fuzz iteration throughput up; the
    // axum router would also reject these, but cheaper to filter
    // before the request build.
    if s.len() > 4096 {
        return;
    }
    // Build a URL with the fuzz input as the segment. URL-encoding
    // any control / non-ASCII chars so `http::Uri` accepts the path.
    // The percent-decoder inside axum will undo this before the
    // extractor sees it — exactly the production code path.
    let encoded: String = s
        .as_bytes()
        .iter()
        .map(|b| {
            // RFC 3986 pchar minus `:` and `/` and `?` and `#`.
            if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
                String::from(*b as char)
            } else {
                format!("%{:02X}", b)
            }
        })
        .collect();
    let uri = format!("/schemas/{}", encoded);
    let Ok(req) = Request::builder().uri(&uri).body(Empty::<axum::body::Bytes>::new()) else {
        return;
    };

    RUNTIME.block_on(async {
        let _ = APP.clone().oneshot(req).await;
    });

    // Sanity: also exercise the underlying decoder directly so the
    // corpus accumulates inputs that touch both surfaces.
    let _ = SchemaId::from_str(s);
});
