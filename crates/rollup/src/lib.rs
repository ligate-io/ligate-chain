//! Ligate Chain rollup binary library.
//!
//! Hosts the [`MockLigateRollup`] blueprint impl. The binary itself
//! lives in `src/main.rs` and is intentionally thin — it just parses
//! CLI args and asks the blueprint to spin up a node.
//!
//! # Why a library + binary split
//!
//! The blueprint impl carries non-trivial trait wiring
//! (`RollupBlueprint`, `FullNodeBlueprint`). Tests need to
//! instantiate it without going through `main`, which is why it's
//! in a library module rather than inlined in `main.rs`. Mirrors the
//! SDK demo-rollup's split (`sov_demo_rollup` lib + `sov-demo-rollup`
//! bin).
//!
//! # Currently supported configurations
//!
//! - **DA layer**: `mock` only. Celestia adapter wiring is Phase A.3.
//! - **zkVM**: `mock` only. Real proving (Risc0 / SP1) is Phase A.4.
//!
//! Production devnet (#13) targets MockDa + MockZkvm too — proving
//! and Celestia are explicitly post-launch concerns. The CLI is
//! shaped to grow into multi-DA / multi-zkVM dispatch later without
//! a breaking change.

#![deny(missing_docs)]
#![allow(clippy::too_many_arguments)]

mod mock_rollup;

pub use mock_rollup::{MockLigateRollup, MockRollupSpec};
