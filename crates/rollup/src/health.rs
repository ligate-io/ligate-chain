//! `/health` and `/ready` HTTP endpoints for `ligate-node`.
//!
//! Tracking issue: #176.
//!
//! Two endpoints, intentionally cheap, served on the chain's main
//! REST surface (the same axum router as `/modules/...`,
//! `/ledger/...`, `/sequencer/...`, NOT the dedicated `/metrics`
//! port from [`crate::metrics`]):
//!
//! - **`GET /health`** is a liveness probe. Returns `200 OK` with
//!   `{"status":"ok"}`. The only path that returns non-200 is "the
//!   process is dead", which axum can't answer (the request times
//!   out at the kernel level, which is exactly the right signal).
//!   k8s `livenessProbe` and systemd unit `WatchdogSec` reference
//!   this.
//!
//! - **`GET /ready`** is a readiness probe. Returns `200 OK` with
//!   `{"status":"synced","synced_da_height":N}` once the node has
//!   caught up to DA tip. While catching up, returns `503 Service
//!   Unavailable` with `{"status":"syncing","synced_da_height":N,
//!   "target_da_height":M}` so dashboards can show progress.
//!   Load balancers + k8s `readinessProbe` reference this to gate
//!   traffic.
//!
//! ## Why two endpoints, not one
//!
//! `/health` is "is the binary alive?" - cheap, always 200 unless
//! the process itself is gone.
//!
//! `/ready` is "should this instance receive traffic?" - encodes
//! the sync state. Asking liveness about readiness conflates "the
//! pod is wedged" with "the pod is just catching up after a
//! restart"; the operations playbook for those is different.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;
use sov_rollup_interface::node::SyncStatus;
use tokio::sync::watch;

/// Shared state for the health handlers. Holds a `watch::Receiver`
/// for the SDK's sync-status broadcast; reading it is non-blocking
/// and zero-cost (the receiver caches the latest value).
#[derive(Clone)]
pub struct HealthState {
    sync_rx: watch::Receiver<SyncStatus>,
}

impl HealthState {
    /// Build a [`HealthState`] from the sync-status receiver the
    /// blueprint hands us in `create_endpoints`.
    pub fn new(sync_rx: watch::Receiver<SyncStatus>) -> Self {
        Self { sync_rx }
    }
}

#[derive(Debug, Clone, Serialize)]
struct HealthBody {
    status: &'static str,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum ReadyBody {
    Synced { synced_da_height: u64 },
    Syncing { synced_da_height: u64, target_da_height: u64 },
}

/// `GET /health`. Always 200 if the axum task is alive.
async fn health() -> Response {
    Json(HealthBody { status: "ok" }).into_response()
}

/// `GET /ready`. 200 if synced, 503 if catching up.
///
/// Reads the latest `SyncStatus` from the receiver without blocking
/// the request handler. The status is updated by the SDK's STF
/// runner on every DA poll, so worst-case staleness is one
/// `da_polling_interval_ms` (default 2s on Celestia, 1s on Mock).
async fn ready(State(state): State<HealthState>) -> Response {
    let status = *state.sync_rx.borrow();
    match status {
        SyncStatus::Synced { synced_da_height } => {
            (StatusCode::OK, Json(ReadyBody::Synced { synced_da_height })).into_response()
        }
        SyncStatus::Syncing { synced_da_height, target_da_height } => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ReadyBody::Syncing { synced_da_height, target_da_height }),
        )
            .into_response(),
    }
}

/// Add `/health` and `/ready` to the given router with the supplied
/// shared state. Keep the layering / state-attachment shape inside
/// this module so the blueprint impl just calls one function.
pub fn add_routes(router: Router, state: HealthState) -> Router {
    router.route("/health", get(health)).route("/ready", get(ready).with_state(state))
}

/// Test seam: build the same router shape `add_routes` produces but
/// from scratch with a caller-supplied `SyncStatus`. Lets the smoke
/// test boot a focused server without standing up the SDK.
#[doc(hidden)]
pub fn router_for_test(initial: SyncStatus) -> (Router, watch::Sender<SyncStatus>) {
    let (tx, rx) = watch::channel(initial);
    (add_routes(Router::new(), HealthState::new(rx)), tx)
}
