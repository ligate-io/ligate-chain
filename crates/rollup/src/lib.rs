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
pub mod cluster;
pub mod health;
pub mod info;
pub mod metrics;
mod mock_rollup;

pub use celestia_rollup::{CelestiaLigateRollup, CelestiaRollupSpec};
pub use chain_config::{ChainConfig, ChainIdError};
pub use mock_rollup::{MockLigateRollup, MockRollupSpec};

// The binary-level `NodeRole` (`Sequencer` vs `Follower`) and its
// follower-only `automatic_batch_production = false` + 503-on-submit
// middleware were removed in chain#446 after a paper-leader incident
// on 2026-05-21: a Follower-mode node promoted to leader via DbElected
// in-process role transition kept the boot-time `false` flag, holding
// the lock but never posting batches. The Sovereign SDK's DbElected
// machinery already gates posting on the Postgres lock and returns
// 503 on POST `/v1/sequencer/txs` when not the leader, so the
// chain-level mode toggle was redundant at best and a footgun at
// worst. If a true read-only RPC node is ever needed, add a proper
// `--no-sequencer` mode that doesn't instantiate the sequencer at
// all, rather than the previous hybrid.
