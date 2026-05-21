//! `GET /v1/cluster/nodes` REST handler.
//!
//! Surfaces the live DbElected cluster topology: every node that has
//! heartbeated into the shared Postgres `nodes` table, plus which one
//! currently holds the `sequencer_leader` lock.
//!
//! Why this exists: after [#435](https://github.com/ligate-io/ligate-chain/issues/435)
//! proved in-process Replica → Leader role transition (1.2s failover on
//! the 2026-05-21 drill), we want a single REST surface that operators,
//! the api proxy, and the explorer can hit instead of dropping into
//! `psql` or Grafana to see who the current leader is. Specified at
//! [#442](https://github.com/ligate-io/ligate-chain/issues/442).
//!
//! Behaviour:
//!
//! - **Nodes in DbElected mode**: serve the live topology from the
//!   `nodes` + `sequencer_leader` tables (single SQL join, cached for
//!   ~1 second).
//! - **Nodes in legacy single-sequencer mode** (no `postgres_config`
//!   in the rollup config): return `503` with `{"reason": "not_clustered"}`.
//!   The chain still works; there's just no cluster to surface.
//!
//! The response includes private VPC IP addresses. **The endpoint
//! MUST be blocked from public access at the Caddy edge** (see
//! `ops/caddy/Caddyfile`, the `handle /v1/cluster/nodes { respond 404 }`
//! block). The api proxy at `api.ligate.io/v1/cluster/nodes` is the
//! public surface and strips addresses there.
//!
//! Tracking issue: [#442](https://github.com/ligate-io/ligate-chain/issues/442).

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Row};
use tokio::sync::RwLock;
use tracing::{debug, error, warn};

/// How long a successful topology response is reusable from cache.
/// Short because the underlying tables move at heartbeat cadence
/// (default 100ms), and a stale response can mislead a failover
/// dashboard about who the current leader is. One second is the
/// sweet spot between absorbing polling and feeling fresh.
const CACHE_TTL: Duration = Duration::from_secs(1);

/// Inner state of the `/v1/cluster/nodes` handler.
///
/// Holds the Postgres pool (or `None` when the node is configured
/// without `[sequencer.preferred.postgres_config]`, i.e. legacy
/// single-sequencer mode), and a small `RwLock`-guarded cache.
///
/// Cloning is cheap: every field is behind an `Arc`. The handler
/// clones this on every request.
#[derive(Clone)]
pub struct ClusterState {
    /// `Some` when the rollup config has a `postgres_config` block;
    /// `None` for legacy single-sequencer mode (the endpoint returns
    /// 503 in that case).
    pool: Option<Arc<PgPool>>,
    /// In-memory cache. Shared across requests so the explorer + ops
    /// dashboards polling at 1Hz don't hammer Postgres.
    cache: Arc<RwLock<Option<(Instant, Arc<ClusterTopology>)>>>,
}

impl ClusterState {
    /// Construct a `ClusterState` with no Postgres backend. The
    /// endpoint will return `503` until reconfigured (which today
    /// means a node restart).
    pub fn disabled() -> Self {
        Self {
            pool: None,
            cache: Arc::new(RwLock::new(None)),
        }
    }

    /// Open a tiny `(max_connections=2)` Postgres pool against the
    /// shared sequencer DB. Failures during pool creation surface
    /// as `Err` so callers can decide whether to fall back to
    /// `disabled()` or refuse to start.
    pub async fn from_connection_string(connection_string: &str) -> anyhow::Result<Self> {
        let pool = PgPoolOptions::new()
            .max_connections(2)
            .acquire_timeout(Duration::from_secs(5))
            .connect(connection_string)
            .await
            .map_err(|e| anyhow::anyhow!("opening cluster Postgres pool: {e}"))?;
        Ok(Self {
            pool: Some(Arc::new(pool)),
            cache: Arc::new(RwLock::new(None)),
        })
    }
}

/// One row of the cluster topology: a single node's identity, the
/// VPC address its peers can reach, the role it advertised on its
/// last heartbeat, and how stale that heartbeat is (milliseconds).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClusterNode {
    /// Stable identifier from the per-VM `postgres_config.node_id`.
    pub node_id: String,
    /// `host:port` form. Private VPC address (e.g. `10.128.0.3:12346`).
    /// The api proxy strips this before forwarding to public callers.
    pub address: String,
    /// Whether this node currently holds the `sequencer_leader` lock.
    pub is_leader: bool,
    /// Milliseconds since this node's last heartbeat into the `nodes`
    /// table. Zero is fresh; values approaching `leader_timeout_millis`
    /// indicate trouble.
    pub last_heartbeat_age_ms: i64,
}

/// Full topology response. The `nodes` list is ordered with the
/// current leader first, then replicas alphabetically by `node_id`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClusterTopology {
    /// Every node that has heartbeated into the cluster's Postgres,
    /// regardless of how stale its heartbeat is. Consumers should
    /// inspect `last_heartbeat_age_ms` per node to decide what counts
    /// as "currently active".
    pub nodes: Vec<ClusterNode>,
    /// Node id of the current leader (the row from `sequencer_leader`),
    /// or `None` if no leader is held right now (which usually means
    /// a failover is in flight; should clear within `leader_timeout_millis`).
    pub leader_node_id: Option<String>,
    /// When the current leader first acquired the lock, as Unix epoch
    /// milliseconds. `None` if no leader is held.
    pub leader_acquired_at_epoch_ms: Option<i64>,
    /// Snapshot time. The same instant for every field above.
    pub generated_at_epoch_ms: i64,
}

/// Error response shape when the endpoint can't serve a topology
/// (either the node isn't clustered, or the database is unreachable).
#[derive(Debug, Serialize)]
struct ClusterErrorBody {
    reason: &'static str,
    detail: Option<String>,
}

/// Build the routes contributed by this module. Mirrors the pattern
/// used by `crate::info::add_routes` so the rollup blueprint nests
/// this under `/v1/` via the existing `Router::nest("/v1", ...)` call
/// at `celestia_rollup.rs:220` and `mock_rollup.rs:233`.
pub fn add_routes<S: Clone + Send + Sync + 'static>(
    router: Router<S>,
    state: ClusterState,
) -> Router<S> {
    router.route("/cluster/nodes", get(handle_cluster_nodes).with_state(state))
}

#[doc(hidden)]
pub fn router_for_test(state: ClusterState) -> Router {
    add_routes(Router::new(), state)
}

/// Build a `ClusterState` from a rollup config. Looks for
/// `[sequencer.preferred.postgres_config]` (the only DbElected-capable
/// shape) and opens a small Postgres pool. Falls back to
/// `disabled()` on any of:
///   - the sequencer is configured as Standard (no Postgres at all),
///   - `postgres_config` is `None`,
///   - opening the Postgres pool fails (warn + continue rather than
///     refuse to boot; the chain still works in single-sequencer mode).
///
/// Shared by `celestia_rollup.rs` and `mock_rollup.rs` so the two
/// rollups stay in lockstep on cluster availability.
pub async fn build_cluster_state<Address, DaService>(
    rollup_config: &sov_stf_runner::RollupConfig<Address, DaService>,
) -> ClusterState
where
    Address: Copy + serde::Serialize + serde::de::DeserializeOwned,
    DaService: sov_rollup_interface::node::da::DaService,
{
    let kind = &rollup_config.sequencer.sequencer_kind_config;
    let postgres_config = match kind {
        sov_full_node_configs::sequencer::SequencerKindConfig::Preferred(cfg) => {
            cfg.postgres_config.as_ref()
        }
        sov_full_node_configs::sequencer::SequencerKindConfig::Standard(_) => None,
    };

    let Some(cfg) = postgres_config else {
        return ClusterState::disabled();
    };

    match ClusterState::from_connection_string(&cfg.postgres_connection_string).await {
        Ok(state) => state,
        Err(err) => {
            tracing::warn!(
                ?err,
                "failed to open cluster Postgres pool; /v1/cluster/nodes will return 503"
            );
            ClusterState::disabled()
        }
    }
}

async fn handle_cluster_nodes(State(state): State<ClusterState>) -> Response {
    let Some(pool) = state.pool.as_ref() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ClusterErrorBody {
                reason: "not_clustered",
                detail: Some(
                    "node is running in legacy single-sequencer mode; \
                     add `[sequencer.preferred.postgres_config]` to celestia.toml \
                     to enable DbElected cluster mode."
                        .to_string(),
                ),
            }),
        )
            .into_response();
    };

    if let Some(cached) = read_cache(&state.cache).await {
        return Json((*cached).clone()).into_response();
    }

    match query_topology(pool).await {
        Ok(topology) => {
            store_cache(&state.cache, topology.clone()).await;
            Json((*topology).clone()).into_response()
        }
        Err(err) => {
            error!(?err, "/v1/cluster/nodes: postgres query failed");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(ClusterErrorBody {
                    reason: "topology_unavailable",
                    detail: Some(format!("{err}")),
                }),
            )
                .into_response()
        }
    }
}

/// Returns a cached topology if it's still inside `CACHE_TTL`.
async fn read_cache(
    cache: &RwLock<Option<(Instant, Arc<ClusterTopology>)>>,
) -> Option<Arc<ClusterTopology>> {
    let guard = cache.read().await;
    let Some((stamped_at, topology)) = guard.as_ref() else {
        return None;
    };
    if stamped_at.elapsed() < CACHE_TTL {
        Some(Arc::clone(topology))
    } else {
        None
    }
}

async fn store_cache(
    cache: &RwLock<Option<(Instant, Arc<ClusterTopology>)>>,
    topology: Arc<ClusterTopology>,
) {
    let mut guard = cache.write().await;
    *guard = Some((Instant::now(), topology));
}

/// Query the shared Postgres for the current cluster topology.
///
/// Single LEFT JOIN: every `nodes` row carries an `is_leader` flag and
/// (when the join lands) the leader's `last_updated` / `leader_acquired_at`
/// columns. Returns leader info from any joined row to keep the SQL
/// shape simple; the values are identical across rows since
/// `sequencer_leader` has a singleton primary key.
async fn query_topology(pool: &PgPool) -> anyhow::Result<Arc<ClusterTopology>> {
    let now = sqlx::query_scalar::<_, sqlx::types::time::OffsetDateTime>("SELECT NOW()")
        .fetch_one(pool)
        .await
        .map_err(|e| anyhow::anyhow!("reading NOW(): {e}"))?;

    let rows = sqlx::query(
        r#"
        SELECT
            n.node_id,
            n.address,
            n.last_updated,
            (n.node_id = l.node_id) AS is_leader,
            l.leader_acquired_at
          FROM nodes n
          LEFT JOIN sequencer_leader l ON l.singleton = 1
         ORDER BY (n.node_id = l.node_id) DESC NULLS LAST, n.node_id ASC
        "#,
    )
    .fetch_all(pool)
    .await
    .map_err(|e| anyhow::anyhow!("listing nodes: {e}"))?;

    let now_unix_ms = now.unix_timestamp_nanos() / 1_000_000;
    let mut nodes = Vec::with_capacity(rows.len());
    let mut leader_node_id: Option<String> = None;
    let mut leader_acquired_at_epoch_ms: Option<i64> = None;

    for row in &rows {
        let node_id: String = row.try_get("node_id")?;
        let address: String = row.try_get("address")?;
        let last_updated: sqlx::types::time::OffsetDateTime = row.try_get("last_updated")?;
        let is_leader: Option<bool> = row.try_get("is_leader")?;
        let is_leader = is_leader.unwrap_or(false);

        let heartbeat_unix_ms = last_updated.unix_timestamp_nanos() / 1_000_000;
        let age_ms = (now_unix_ms - heartbeat_unix_ms) as i64;

        if is_leader {
            leader_node_id = Some(node_id.clone());
            let acquired_at: Option<sqlx::types::time::OffsetDateTime> =
                row.try_get("leader_acquired_at").ok();
            leader_acquired_at_epoch_ms =
                acquired_at.map(|t| (t.unix_timestamp_nanos() / 1_000_000) as i64);
        }

        nodes.push(ClusterNode {
            node_id,
            address,
            is_leader,
            last_heartbeat_age_ms: age_ms,
        });
    }

    if leader_node_id.is_none() {
        debug!(
            "/v1/cluster/nodes: no leader row in sequencer_leader \
             (cluster may be mid-failover or just bootstrapped)"
        );
    }
    if nodes.is_empty() {
        warn!(
            "/v1/cluster/nodes: nodes table is empty; this node has \
             not yet completed its first heartbeat"
        );
    }

    Ok(Arc::new(ClusterTopology {
        nodes,
        leader_node_id,
        leader_acquired_at_epoch_ms,
        generated_at_epoch_ms: now_unix_ms as i64,
    }))
}

/// Convenience: best-effort coerce `i128` epoch-ms into `i64`, with a
/// sentinel-floor for the rare case where the JSON consumer wants
/// "unknown time" rather than a panic on overflow. Unused today but
/// kept here for the explorer's potential client-side display logic.
#[doc(hidden)]
pub fn _epoch_ms_floor(v: i128) -> i64 {
    v.try_into().unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "multi_thread")]
    async fn disabled_state_returns_503() {
        let state = ClusterState::disabled();
        let app: Router<()> = add_routes(Router::new(), state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve");
        });

        let url = format!("http://{addr}/cluster/nodes");
        let resp = reqwest::get(&url).await.expect("get");
        assert_eq!(resp.status().as_u16(), 503);
        let body: serde_json::Value = resp.json().await.expect("json");
        assert_eq!(body["reason"], "not_clustered");
        assert!(body["detail"]
            .as_str()
            .unwrap_or("")
            .contains("postgres_config"));
    }
}
