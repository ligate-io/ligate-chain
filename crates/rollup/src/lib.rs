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
pub mod metrics;
mod mock_rollup;

pub use celestia_rollup::{CelestiaLigateRollup, CelestiaRollupSpec};
pub use mock_rollup::{MockLigateRollup, MockRollupSpec};
