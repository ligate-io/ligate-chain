//! Ligate Chain rollup binary library.
//!
//! Hosts the blueprint impls. The binary itself lives in
//! `src/main.rs` and is intentionally thin — it just parses CLI
//! args and asks the right blueprint to spin up a node.
//!
//! # Why a library + binary split
//!
//! The blueprint impls carry non-trivial trait wiring
//! (`RollupBlueprint`, `FullNodeBlueprint`). Tests need to
//! instantiate them without going through `main`, which is why
//! they're in library modules rather than inlined in `main.rs`.
//! Mirrors the SDK demo-rollup's split (`sov_demo_rollup` lib +
//! `sov-demo-rollup` bin).
//!
//! # Currently supported configurations
//!
//! | DA layer | Status | Notes |
//! |---|---|---|
//! | `mock` | available | In-process SQLite-backed mock DA. Single-node only. |
//! | `celestia` | available | Real Celestia DA (Mocha testnet or local light node). |
//!
//! Both DA layers run against MockZkvm for now — real proving
//! (Risc0 / SP1) lands in Phase A.4 and is a small blueprint diff
//! when it does (swap the `Spec` alias + `create_prover_service`
//! body, leave everything else).

#![deny(missing_docs)]
#![allow(clippy::too_many_arguments)]

mod celestia_rollup;
pub mod chain_config;
pub mod follower_guard;
pub mod health;
pub mod info;
pub mod metrics;
mod mock_rollup;

pub use celestia_rollup::{CelestiaLigateRollup, CelestiaRollupSpec};
pub use chain_config::{ChainConfig, ChainIdError};
pub use mock_rollup::{MockLigateRollup, MockRollupSpec};

/// Operator role this node should run as.
///
/// `Sequencer` is the normal full-node-with-sequencer flow; the node
/// builds batches and posts them to DA. `Follower` is a read-only
/// sync mode for non-sequencer operators (design partners, auditors,
/// anyone running their own Ligate node who isn't the registered
/// sequencer).
///
/// In `Follower` mode `main.rs` disables `automatic_batch_production`
/// (no posts to DA), and `create_endpoints` layers a middleware
/// (see [`follower_guard`]) that returns `503 Service Unavailable`
/// for `POST /v1/sequencer/txs` so submissions don't silently
/// disappear into a local mempool that never propagates.
///
/// Tracking: [#243](https://github.com/ligate-io/ligate-chain/issues/243).
///
/// # SEAM: multi-sequencer leader assignment
///
/// This binary-level [`NodeRole`] (`Sequencer` vs `Follower`) is
/// coarse; multi-sequencer (#82) needs a *slot-level* "who is the
/// active leader right now?" decision among N peer sequencers.
///
/// **The Sovereign SDK already implements this upstream.** It is a
/// configuration concern, not a code-change concern, for our chain:
///
/// - The actual leader / replica branch lives in the SDK fork at
///   `sov-sequencer/src/preferred/db/mod.rs:543-564`, matching on
///   [`sov_full_node_configs::sequencer::ConfiguredNodeRole`]
///   (`Leader` / `Replica` / `ReplicaNoLeaderSync` / `DbElected`).
/// - `DbElected` mode runs a Postgres-backed election with heartbeat
///   timeout, grace period, and replica state-sync. See `db/heartbeat_task.rs`
///   and `replica/replica_sync_task.rs` in the SDK fork.
/// - To activate it for our chain, [`PreferredSequencerConfig::postgres_config`]
///   needs to be set in `devnet/rollup.toml`'s `[sequencer.preferred]`
///   section (currently absent, which is why we run single-instance).
///
/// The design-doc sub-issues for Option 1 collapse:
/// - Sub-issue 2 (LeaderSchedule) and 3 (view-change): **already
///   shipped upstream**; we use `DbElected` instead of writing our own.
/// - Sub-issue 4 (multi-instance prod deploy), 5 (force-include
///   observability stub), and 6 (docs + runbook) are still required.
/// - New requirement: provision a managed Postgres instance accessible
///   from every sequencer VM (Cloud SQL or self-hosted on the same VPC).
///
/// See `docs/protocol/sequencer-decentralisation.md` §5 for the
/// updated implementation plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeRole {
    /// Normal full node: produces batches, posts to DA, accepts
    /// submissions on `/v1/sequencer/txs`.
    Sequencer,
    /// Read-only follower: syncs STF from DA, serves read endpoints,
    /// does not auto-produce batches, returns 503 on submissions.
    Follower,
}
