//! Chain identity configuration: the `[chain]` section of the rollup
//! TOML.
//!
//! Lives next to `[storage]`, `[runner]`, `[da]`, `[sequencer]`, etc.
//! in the same `devnet/*.toml` file. Logically distinct from the SDK's
//! `RollupConfig` because the SDK doesn't model chain identity, the
//! ladder is a Ligate convention layered on top.
//!
//! ## Why this is its own module
//!
//! The SDK's `RollupConfig` has `#[serde(deny_unknown_fields)]`, so
//! `[chain]` can't simply ride along inside the same struct. The
//! [`load_split_config`] helper does a two-pass parse: pull `[chain]`
//! out into [`ChainConfig`], then hand the rest to the SDK loader.
//!
//! ## What it does for v0
//!
//! Two things, both operator-facing:
//!
//! 1. Boot-time ladder validation. A node booted with
//!    `chain_id = "ligate-1.0"` (or any non-ladder value) refuses to
//!    start with a typed [`ChainIdError`]. Operators running multiple
//!    networks side-by-side can't silently boot the wrong one.
//! 2. REST surface. `GET /v1/rollup/info` returns the configured
//!    `chain_id` + the runtime's `CHAIN_HASH` + the binary version,
//!    so wallets and explorers can identify the network without
//!    out-of-band knowledge.
//!
//! Tracking issue: [#181](https://github.com/ligate-io/ligate-chain/issues/181).

use std::path::Path;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Errors raised while loading or validating the `[chain]` section.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ChainIdError {
    /// The TOML file couldn't be read from disk.
    #[error("failed to read rollup config from {path}: {source}")]
    Io {
        /// Path that failed to read.
        path: String,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// The TOML file parsed but didn't have a `[chain]` section.
    #[error("missing [chain] section in {path}")]
    MissingSection {
        /// Path that lacked the section.
        path: String,
    },
    /// `[chain]` parsed but `chain_id` was missing or non-string.
    #[error("missing or non-string chain_id in [chain] section of {path}")]
    MissingChainId {
        /// Path with the malformed `[chain]`.
        path: String,
    },
    /// The TOML couldn't be parsed at all.
    #[error("failed to parse {path}: {source}")]
    ParseToml {
        /// Path that failed to parse.
        path: String,
        /// Underlying TOML error.
        #[source]
        source: toml::de::Error,
    },
    /// The TOML couldn't be re-serialized after stripping `[chain]`.
    #[error("failed to re-serialize stripped TOML for {path}: {source}")]
    ReserializeToml {
        /// Path that failed to re-serialize.
        path: String,
        /// Underlying TOML error.
        #[source]
        source: toml::ser::Error,
    },
    /// `chain_id` doesn't match the locked ladder format.
    ///
    /// See [`validate_chain_id`] for the accepted shapes.
    #[error(
        "chain_id '{got}' does not match the locked ladder. \
         Accepted: 'ligate-localnet', 'ligate-devnet-N', \
         'ligate-testnet-N', or 'ligate-N' where N is a positive integer"
    )]
    InvalidLadder {
        /// Value the operator supplied.
        got: String,
    },
}

/// The `[chain]` section of the rollup config TOML.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ChainConfig {
    /// Cosmos-style string chain identifier. Must match the locked
    /// ladder per [`validate_chain_id`].
    pub chain_id: String,
}

/// Validate `chain_id` against the locked Ligate ladder.
///
/// Accepted shapes:
///
/// - `ligate-localnet` (single-node local dev)
/// - `ligate-devnet-N` where `N` is a positive integer (public devnet)
/// - `ligate-testnet-N` where `N` is a positive integer (public testnet)
/// - `ligate-N` where `N` is a positive integer (mainnet)
///
/// Trailing number bumps only on a chain restart that breaks state
/// continuity. Soft upgrades via the upgrade module keep the same id.
/// `0x` prefix is never correct (chain ids are bare strings, not hex).
pub fn validate_chain_id(s: &str) -> Result<(), ChainIdError> {
    if s == "ligate-localnet" {
        return Ok(());
    }
    let suffix = s
        .strip_prefix("ligate-")
        .ok_or_else(|| ChainIdError::InvalidLadder { got: s.to_string() })?;

    // Suffix is one of:
    //   "devnet-N"    -> network with iteration counter
    //   "testnet-N"   -> same shape
    //   "N"           -> mainnet
    let numeric_part = suffix
        .strip_prefix("devnet-")
        .or_else(|| suffix.strip_prefix("testnet-"))
        .unwrap_or(suffix);

    // Reject empty (would happen for "ligate-devnet-" or just "ligate-")
    // and zero, plus anything non-numeric.
    let n: u32 =
        numeric_part.parse().map_err(|_| ChainIdError::InvalidLadder { got: s.to_string() })?;
    if n == 0 {
        return Err(ChainIdError::InvalidLadder { got: s.to_string() });
    }
    Ok(())
}

/// Load a rollup TOML file, splitting `[chain]` from the SDK's
/// `RollupConfig`.
///
/// Two-pass:
///
/// 1. Parse the file as a `toml::Value`. Pull out `[chain]` and parse
///    it into a [`ChainConfig`]. Validate `chain_id` against the
///    ladder.
/// 2. Re-serialize the remainder (without `[chain]`) and return both
///    the [`ChainConfig`] and the raw TOML string for the caller to
///    pass to the SDK's `from_toml_str` (or equivalent).
///
/// The split is needed because the SDK's `RollupConfig` carries
/// `#[serde(deny_unknown_fields)]`, so a `[chain]` table in the same
/// file would otherwise reject deserialization.
pub fn load_split_config(path: impl AsRef<Path>) -> Result<(ChainConfig, String), ChainIdError> {
    let path_ref = path.as_ref();
    let path_str = path_ref.display().to_string();
    let text = std::fs::read_to_string(path_ref)
        .map_err(|source| ChainIdError::Io { path: path_str.clone(), source })?;

    let mut value: toml::Value = toml::from_str(&text)
        .map_err(|source| ChainIdError::ParseToml { path: path_str.clone(), source })?;

    let chain_value = value
        .as_table_mut()
        .and_then(|t| t.remove("chain"))
        .ok_or_else(|| ChainIdError::MissingSection { path: path_str.clone() })?;

    let chain: ChainConfig = chain_value
        .try_into()
        .map_err(|_| ChainIdError::MissingChainId { path: path_str.clone() })?;
    validate_chain_id(&chain.chain_id)?;

    let stripped = toml::to_string(&value)
        .map_err(|source| ChainIdError::ReserializeToml { path: path_str, source })?;

    Ok((chain, stripped))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_accepts_localnet() {
        assert!(validate_chain_id("ligate-localnet").is_ok());
    }

    #[test]
    fn validate_accepts_devnet_one() {
        assert!(validate_chain_id("ligate-devnet-1").is_ok());
    }

    #[test]
    fn validate_accepts_devnet_higher_iteration() {
        assert!(validate_chain_id("ligate-devnet-42").is_ok());
    }

    #[test]
    fn validate_accepts_testnet() {
        assert!(validate_chain_id("ligate-testnet-1").is_ok());
        assert!(validate_chain_id("ligate-testnet-7").is_ok());
    }

    #[test]
    fn validate_accepts_mainnet() {
        assert!(validate_chain_id("ligate-1").is_ok());
        assert!(validate_chain_id("ligate-2").is_ok());
    }

    #[test]
    fn validate_rejects_hex_prefix() {
        // The original audit-trigger pattern from issue #54: a non-hex
        // string masquerading as hex.
        let err = validate_chain_id("0x1G8").unwrap_err();
        assert!(matches!(err, ChainIdError::InvalidLadder { .. }));
    }

    #[test]
    fn validate_rejects_dot_separated_version() {
        // A common typo: treating chain_id like a semver string.
        let err = validate_chain_id("ligate-1.0").unwrap_err();
        assert!(matches!(err, ChainIdError::InvalidLadder { .. }));
    }

    #[test]
    fn validate_rejects_empty() {
        let err = validate_chain_id("").unwrap_err();
        assert!(matches!(err, ChainIdError::InvalidLadder { .. }));
    }

    #[test]
    fn validate_rejects_zero_iteration() {
        // Trailing-zero is reserved (never a real network); reject so
        // operators don't mistake "ligate-0" for "fresh local devnet".
        let err = validate_chain_id("ligate-0").unwrap_err();
        assert!(matches!(err, ChainIdError::InvalidLadder { .. }));
        let err = validate_chain_id("ligate-devnet-0").unwrap_err();
        assert!(matches!(err, ChainIdError::InvalidLadder { .. }));
    }

    #[test]
    fn validate_rejects_missing_iteration_suffix() {
        let err = validate_chain_id("ligate-devnet-").unwrap_err();
        assert!(matches!(err, ChainIdError::InvalidLadder { .. }));
        let err = validate_chain_id("ligate-").unwrap_err();
        assert!(matches!(err, ChainIdError::InvalidLadder { .. }));
    }

    #[test]
    fn validate_rejects_unknown_network_word() {
        // "staging" or "preview" aren't in the ladder. Operators
        // should bump the ladder via spec amendment, not invent
        // off-roster names.
        let err = validate_chain_id("ligate-staging-1").unwrap_err();
        assert!(matches!(err, ChainIdError::InvalidLadder { .. }));
    }

    #[test]
    fn validate_rejects_wrong_brand_prefix() {
        // A sister Sovereign-SDK chain id pattern that isn't ours.
        let err = validate_chain_id("celestia-mocha-4").unwrap_err();
        assert!(matches!(err, ChainIdError::InvalidLadder { .. }));
    }

    #[test]
    fn load_split_config_round_trips() {
        // End-to-end: write a TOML file with [chain] alongside
        // unrelated sections, parse, ensure [chain] is stripped from
        // the residual and the rest survives intact.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("rollup.toml");
        std::fs::write(
            &path,
            r#"
[chain]
chain_id = "ligate-localnet"

[storage]
path = "/tmp/storage"

[runner]
da_polling_interval_ms = 100
"#,
        )
        .expect("write");

        let (chain, residual) = load_split_config(&path).expect("split");
        assert_eq!(chain.chain_id, "ligate-localnet");
        assert!(!residual.contains("[chain]"), "[chain] survived strip");
        assert!(residual.contains("[storage]"), "storage section dropped");
        assert!(residual.contains("[runner]"), "runner section dropped");
    }

    #[test]
    fn load_split_config_rejects_missing_chain_section() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("rollup.toml");
        std::fs::write(&path, "[storage]\npath = \"/tmp\"\n").expect("write");
        let err = load_split_config(&path).unwrap_err();
        assert!(matches!(err, ChainIdError::MissingSection { .. }));
    }

    #[test]
    fn load_split_config_rejects_invalid_ladder() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("rollup.toml");
        std::fs::write(
            &path,
            r#"
[chain]
chain_id = "ligate-1.0"

[storage]
path = "/tmp"
"#,
        )
        .expect("write");
        let err = load_split_config(&path).unwrap_err();
        assert!(matches!(err, ChainIdError::InvalidLadder { .. }));
    }
}
