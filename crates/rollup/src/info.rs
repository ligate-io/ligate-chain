//! `GET /v1/rollup/info` REST handler.
//!
//! Surfaces three operator- and wallet-facing identifiers in a single
//! response:
//!
//! - `chain_id`: the Cosmos-style string from the rollup config's
//!   `[chain]` section (e.g. `ligate-localnet`, `ligate-devnet-1`).
//!   Bumps on state-breaking restarts only; stable across STF
//!   upgrades.
//! - `chain_hash`: the runtime's build-script-generated 32-byte
//!   `CHAIN_HASH`, hex-encoded. Bumps on every runtime composition
//!   change (added module, changed const, etc.). Used for tx
//!   replay-protection in the signing domain.
//! - `version`: the `ligate-node` binary version (`CARGO_PKG_VERSION`).
//!
//! Why three: `chain_id` is the human-readable network identifier that
//! wallets and explorers display. `chain_hash` is the cryptographic
//! identifier that pins the exact runtime state-transition logic.
//! Different roles, both exposed so consumers don't have to choose.
//!
//! Tracking issue: [#181](https://github.com/ligate-io/ligate-chain/issues/181).

use std::sync::Arc;

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

/// Inner state of the `/v1/rollup/info` handler.
///
/// Cloned by axum on every request (cheap, since `chain_id` is shared
/// behind an `Arc`). Holding `Arc<str>` instead of `String` makes the
/// `Clone` zero-allocation; over a busy `/info` poll loop this matters.
#[derive(Clone)]
pub struct InfoState {
    chain_id: Arc<str>,
    chain_hash: [u8; 32],
    version: &'static str,
}

impl InfoState {
    /// Build a new `InfoState` from the configured chain identity.
    pub fn new(chain_id: impl Into<Arc<str>>, chain_hash: [u8; 32]) -> Self {
        Self { chain_id: chain_id.into(), chain_hash, version: env!("CARGO_PKG_VERSION") }
    }
}

/// Response body of `GET /v1/rollup/info`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RollupInfo {
    /// Cosmos-style chain identifier from the `[chain]` config section.
    pub chain_id: String,
    /// Runtime's build-time `CHAIN_HASH`, hex-encoded (lowercase, no
    /// `0x` prefix, 64 chars).
    pub chain_hash: String,
    /// `ligate-node` binary version (semver per `CARGO_PKG_VERSION`).
    pub version: String,
}

/// Build the routes contributed by this module.
///
/// Mounted at `/rollup/info` (the chain blueprint nests this under the
/// `/v1/` prefix per #149).
pub fn add_routes<S: Clone + Send + Sync + 'static>(
    router: Router<S>,
    state: InfoState,
) -> Router<S> {
    router.route("/rollup/info", get(handle_info).with_state(state))
}

/// Test seam: build a freestanding router with the info routes
/// mounted at `/rollup/info`, ready to hand to `axum::serve`.
///
/// Mirrors `crate::health::router_for_test` so smoke tests don't have
/// to repeat the `Router::new() + add_routes(...)` boilerplate. Used
/// by `tests/info_smoke.rs`.
#[doc(hidden)]
pub fn router_for_test(chain_id: impl Into<std::sync::Arc<str>>, chain_hash: [u8; 32]) -> Router {
    add_routes(Router::new(), InfoState::new(chain_id, chain_hash))
}

async fn handle_info(State(state): State<InfoState>) -> Json<RollupInfo> {
    Json(RollupInfo {
        chain_id: state.chain_id.to_string(),
        chain_hash: hex_encode_lower(&state.chain_hash),
        version: state.version.to_string(),
    })
}

/// Lowercase hex encode, no separators, no `0x` prefix.
///
/// Inlined rather than pulling the `hex` crate; the chain_hash is the
/// only place we need it on this code path.
fn hex_encode_lower(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        // The two-char `{:02x}` form left-pads single-digit values
        // with a `0`. Without the `02`, `0x0a` would render as `a`
        // and break the fixed-width hex contract.
        use std::fmt::Write as _;
        let _ = write!(out, "{b:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_encode_lower_pads_single_digits() {
        // 0x0A must render as "0a", not "a", so a 32-byte hash always
        // produces 64 chars regardless of byte values.
        assert_eq!(hex_encode_lower(&[0x0a]), "0a");
        assert_eq!(hex_encode_lower(&[0xff, 0x00, 0x10]), "ff0010");
    }

    #[test]
    fn hex_encode_lower_round_trips_zero_bytes() {
        let zeros = [0u8; 32];
        let s = hex_encode_lower(&zeros);
        assert_eq!(s.len(), 64);
        assert!(s.chars().all(|c| c == '0'));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn info_endpoint_returns_configured_values() {
        // Spawn the router on an ephemeral port and hit `/rollup/info`
        // with a real HTTP client. Verifies the wire shape end-to-end:
        // chain_id from state, chain_hash hex-encoded, version from
        // env.
        let state = InfoState::new("ligate-localnet", [0xABu8; 32]);
        let app: Router<()> = add_routes(Router::new(), state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve");
        });

        let url = format!("http://{addr}/rollup/info");
        let resp = reqwest::get(&url).await.expect("get");
        assert_eq!(resp.status().as_u16(), 200);
        let body: RollupInfo = resp.json().await.expect("json");
        assert_eq!(body.chain_id, "ligate-localnet");
        assert_eq!(
            body.chain_hash,
            "abababababababababababababababababababababababababababababababab"
        );
        assert_eq!(body.version, env!("CARGO_PKG_VERSION"));
    }
}
