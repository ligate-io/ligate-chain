//! Genesis configuration assembly.
//!
//! Operators bring a chain up by handing the node a directory of
//! per-module JSON files. [`create_genesis_config`] reads each file,
//! deserialises it into the matching module's `*Config` type, and
//! returns the runtime's derive-generated [`GenesisConfig`]. Before
//! returning, it runs [`validate_config`] to catch cross-module
//! inconsistencies that the per-module deserialisers can't see — most
//! importantly, that the attestation module's `$LGT` token id matches
//! the bank's gas token id.
//!
//! # JSON layout
//!
//! Conventional layout — the format the rollup binary will assume in
//! the next slice — is a single directory holding one JSON per
//! module:
//!
//! ```text
//! devnet/
//!   bank.json
//!   accounts.json
//!   sequencer_registry.json
//!   operator_incentives.json
//!   attester_incentives.json
//!   prover_incentives.json
//!   chain_state.json
//!   attestation.json
//! ```
//!
//! The `uniqueness` and `blob_storage` modules use `Config = ()` and
//! need no file. [`GenesisPaths::from_dir`] expands a directory path
//! into the full set.
//!
//! # Validation
//!
//! [`validate_config`] is the only place cross-module invariants are
//! checked. Today it enforces two things:
//!
//! 1. `bank.gas_token_config` is `Some(...)`. A chain without a gas
//!    token can't charge fees, can't run, and is almost certainly the
//!    result of a config typo.
//! 2. `attestation.lgt_token_id` equals [`sov_bank::config_gas_token_id`].
//!    Attestation fees are denominated in `$LGT`; if the two ids
//!    disagree, fee charges would route to a token nobody holds and
//!    every attestation submission would revert.
//!
//! New invariants land here as we add modules.

use std::path::{Path, PathBuf};
use std::{fs, io};

use serde::de::DeserializeOwned;
use serde::Serialize;
use sov_address::{EthereumAddress, FromVmAddress};
use sov_bank::config_gas_token_id;
use sov_modules_api::Spec;
use thiserror::Error;

use crate::runtime::GenesisConfig;

/// Per-module genesis JSON file paths.
///
/// Convention: a single directory holds one JSON per module, named
/// `<module>.json`. [`GenesisPaths::from_dir`] expands a directory
/// path into the full set. Constructing the struct field-by-field is
/// also fine if you want files spread across multiple directories
/// (useful for tests).
#[derive(Clone, Debug)]
pub struct GenesisPaths {
    /// `sov_bank::BankConfig` JSON.
    pub bank: PathBuf,
    /// `sov_accounts::AccountConfig` JSON.
    pub accounts: PathBuf,
    /// `sov_sequencer_registry::SequencerRegistryConfig` JSON.
    pub sequencer_registry: PathBuf,
    /// `sov_operator_incentives::OperatorIncentivesConfig` JSON.
    pub operator_incentives: PathBuf,
    /// `sov_attester_incentives::AttesterIncentivesConfig` JSON.
    pub attester_incentives: PathBuf,
    /// `sov_prover_incentives::ProverIncentivesConfig` JSON.
    pub prover_incentives: PathBuf,
    /// `sov_chain_state::ChainStateConfig` JSON.
    pub chain_state: PathBuf,
    /// `attestation::AttestationConfig` JSON.
    pub attestation: PathBuf,
}

impl GenesisPaths {
    /// Conventional layout: a directory holding `bank.json`,
    /// `accounts.json`, etc.
    pub fn from_dir(dir: impl AsRef<Path>) -> Self {
        let dir = dir.as_ref();
        Self {
            bank: dir.join("bank.json"),
            accounts: dir.join("accounts.json"),
            sequencer_registry: dir.join("sequencer_registry.json"),
            operator_incentives: dir.join("operator_incentives.json"),
            attester_incentives: dir.join("attester_incentives.json"),
            prover_incentives: dir.join("prover_incentives.json"),
            chain_state: dir.join("chain_state.json"),
            attestation: dir.join("attestation.json"),
        }
    }
}

/// Errors raised while assembling or validating a [`GenesisConfig`].
///
/// `Io` and `ParseJson` keep the offending path in the error so
/// operators can pinpoint the bad file without scanning logs.
#[derive(Debug, Error)]
pub enum GenesisError {
    /// Failed to read a per-module JSON file from disk.
    #[error("failed to read {path}: {source}")]
    Io {
        /// Path that failed to read.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: io::Error,
    },
    /// The per-module JSON parsed but didn't match the module's
    /// `*Config` schema.
    #[error("failed to parse {path}: {source}")]
    ParseJson {
        /// Path whose JSON failed to deserialise.
        path: PathBuf,
        /// Underlying serde error.
        #[source]
        source: serde_json::Error,
    },
    /// `bank.gas_token_config` is `None`. A chain without a gas
    /// token can't charge fees and can't run.
    #[error("bank.gas_token_config is unset; chain has no gas token")]
    MissingGasToken,
    /// `attestation.lgt_token_id` disagrees with the bank's gas
    /// token id. Every attestation submission would revert.
    #[error("attestation.lgt_token_id ({attestation}) must equal bank's gas token id ({bank})")]
    LgtTokenIdMismatch {
        /// The id stored in `AttestationConfig`.
        attestation: String,
        /// The id derived from [`sov_bank::config_gas_token_id`].
        bank: String,
    },
}

/// Read each module's JSON from `paths`, assemble the runtime's
/// [`GenesisConfig`], and run [`validate_config`].
///
/// This is the function the rollup binary will call at startup. The
/// `S::Address` bound is the runtime's bound plus `Serialize +
/// DeserializeOwned`, which `sov_bank::BankConfig` and friends
/// require to round-trip addresses through JSON.
pub fn create_genesis_config<S: Spec>(
    paths: &GenesisPaths,
) -> Result<GenesisConfig<S>, GenesisError>
where
    S::Address: FromVmAddress<EthereumAddress> + Serialize + DeserializeOwned,
{
    let cfg = GenesisConfig {
        bank: read_json(&paths.bank)?,
        accounts: read_json(&paths.accounts)?,
        sequencer_registry: read_json(&paths.sequencer_registry)?,
        operator_incentives: read_json(&paths.operator_incentives)?,
        attester_incentives: read_json(&paths.attester_incentives)?,
        prover_incentives: read_json(&paths.prover_incentives)?,
        uniqueness: (),
        chain_state: read_json(&paths.chain_state)?,
        blob_storage: (),
        attestation: read_json(&paths.attestation)?,
    };
    validate_config(&cfg)?;
    Ok(cfg)
}

/// Run cross-module invariants on an already-loaded [`GenesisConfig`].
///
/// Exposed publicly so callers can dry-run a configuration assembled
/// in code (e.g. tests, in-process configs) without touching the
/// filesystem. [`create_genesis_config`] calls this internally before
/// returning.
pub fn validate_config<S: Spec>(cfg: &GenesisConfig<S>) -> Result<(), GenesisError>
where
    S::Address: FromVmAddress<EthereumAddress>,
{
    if cfg.bank.gas_token_config.is_none() {
        return Err(GenesisError::MissingGasToken);
    }

    let bank_token = config_gas_token_id();
    if cfg.attestation.lgt_token_id != bank_token {
        return Err(GenesisError::LgtTokenIdMismatch {
            attestation: cfg.attestation.lgt_token_id.to_string(),
            bank: bank_token.to_string(),
        });
    }

    Ok(())
}

fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T, GenesisError> {
    let bytes =
        fs::read(path).map_err(|source| GenesisError::Io { path: path.to_path_buf(), source })?;
    serde_json::from_slice(&bytes)
        .map_err(|source| GenesisError::ParseJson { path: path.to_path_buf(), source })
}
