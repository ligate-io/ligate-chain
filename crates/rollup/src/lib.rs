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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeRole {
    /// Normal full node: produces batches, posts to DA, accepts
    /// submissions on `/v1/sequencer/txs`.
    Sequencer,
    /// Read-only follower: syncs STF from DA, serves read endpoints,
    /// does not auto-produce batches, returns 503 on submissions.
    Follower,
}
