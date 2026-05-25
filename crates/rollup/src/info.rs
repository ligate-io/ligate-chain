//! `GET /v1/rollup/info` REST handler.
//!
//! Surfaces three operator- and wallet-facing identifiers in a single
//! response:
//!
//! - `chain_id`: the Cosmos-style string from the rollup config's
//!   `[chain]` section (e.g. `ligate-localnet`, `ligate-devnet-2`).
//!   Bumps on state-breaking restarts only; stable across STF
//!   upgrades.
//! - `chain_hash`: the runtime's build-script-generated 32-byte
//!   `CHAIN_HASH`, bech32m-encoded with HRP `lsch` (`lsch1...`).
//!   Matches the format used by the SDK's `/v1/rollup/schema`
//!   endpoint so partners see the same identifier on both surfaces.
//!   Bumps on every runtime composition change (added module, changed
//!   const, etc.); used for tx replay-protection in the signing domain.
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
use sov_rollup_interface::ChainHash;

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
    /// Runtime's build-time `CHAIN_HASH`, bech32m-encoded with HRP
    /// `lsch` (`lsch1...`). Matches the SDK's `/v1/rollup/schema`
    /// response shape so wallets see one identifier across both
    /// surfaces.
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
        chain_hash: ChainHash::new(state.chain_hash).to_string(),
        version: state.version.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "multi_thread")]
    async fn info_endpoint_returns_configured_values() {
        // Spawn the router on an ephemeral port and hit `/rollup/info`
        // with a real HTTP client. Verifies the wire shape end-to-end:
        // chain_id from state, chain_hash bech32m-encoded, version
        // from env.
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
        // bech32m with HRP `lsch`. Decoding it back must yield the
        // same 32 bytes we configured.
        assert!(body.chain_hash.starts_with("lsch1"), "got {}", body.chain_hash);
        let parsed: ChainHash = body.chain_hash.parse().expect("parse bech32m");
        assert_eq!(parsed.0, [0xABu8; 32]);
        assert_eq!(body.version, env!("CARGO_PKG_VERSION"));
    }
}
