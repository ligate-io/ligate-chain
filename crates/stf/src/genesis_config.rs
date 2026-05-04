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
#[non_exhaustive]
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

    /// `InitialAttestorSet.members` is empty. The runtime's
    /// `RegisterAttestorSet` handler rejects this; the genesis loader
    /// should fail at the same boundary so a misconfigured genesis
    /// doesn't write garbage state.
    #[error("initial_attestor_sets[{index}] has empty members vec")]
    InitialAttestorSetEmpty {
        /// Index in `attestation.initial_attestor_sets`.
        index: usize,
    },

    /// `InitialAttestorSet.threshold` is 0 or larger than the member
    /// count. Mirrors the `ZeroThreshold` / `ThresholdExceedsMembers`
    /// runtime errors.
    #[error("initial_attestor_sets[{index}] has threshold {threshold} outside 1..={count}")]
    InitialAttestorSetInvalidThreshold {
        /// Index in `attestation.initial_attestor_sets`.
        index: usize,
        /// Supplied threshold.
        threshold: u8,
        /// Number of members supplied.
        count: usize,
    },

    /// Two `InitialAttestorSet` entries derive to the same
    /// `AttestorSetId`. Pointless duplication; the runtime's
    /// `DuplicateAttestorSet` rejection would fire on the second
    /// genesis-time registration.
    #[error("initial_attestor_sets[{index}] duplicates the id derived from index {first}")]
    InitialAttestorSetDuplicate {
        /// First index that produced the colliding id.
        first: usize,
        /// Second (rejected) index.
        index: usize,
    },

    /// `InitialSchema.attestor_set` references an `AttestorSetId`
    /// that no `InitialAttestorSet` entry derives to. The schema
    /// would be unverifiable from genesis (no attestors registered
    /// for it).
    #[error(
        "initial_schemas[{index}] (name={name:?}) references attestor_set {attestor_set} \
         which is not registered by any initial_attestor_sets entry"
    )]
    InitialSchemaUnknownAttestorSet {
        /// Index in `attestation.initial_schemas`.
        index: usize,
        /// Schema name (for operator triage).
        name: String,
        /// The `AttestorSetId` that didn't resolve.
        attestor_set: String,
    },

    /// Two `InitialSchema` entries derive to the same `SchemaId`.
    /// Same `(owner, name, version)` tuple. The runtime's
    /// `DuplicateSchema` rejection would fire on the second
    /// genesis-time registration.
    #[error("initial_schemas[{index}] duplicates the id derived from index {first}")]
    InitialSchemaDuplicate {
        /// First index that produced the colliding id.
        first: usize,
        /// Second (rejected) index.
        index: usize,
    },

    /// `InitialSchema.fee_routing_bps` is non-zero with no
    /// `fee_routing_addr`, or vice versa. Mirrors the runtime's
    /// `OrphanFeeRouting` rejection.
    #[error(
        "initial_schemas[{index}] (name={name:?}) has orphan fee routing: \
         bps={bps}, addr={has_addr}"
    )]
    InitialSchemaOrphanRouting {
        /// Index in `attestation.initial_schemas`.
        index: usize,
        /// Schema name (for operator triage).
        name: String,
        /// Configured basis points.
        bps: u16,
        /// Whether `fee_routing_addr` was set.
        has_addr: bool,
    },

    /// `InitialSchema.fee_routing_bps` exceeds the genesis-configured
    /// `max_builder_bps` cap. Mirrors the runtime's
    /// `FeeRoutingExceedsCap` rejection.
    #[error("initial_schemas[{index}] (name={name:?}) fee_routing_bps {bps} exceeds cap {cap}")]
    InitialSchemaFeeRoutingExceedsCap {
        /// Index in `attestation.initial_schemas`.
        index: usize,
        /// Schema name (for operator triage).
        name: String,
        /// Supplied basis points.
        bps: u16,
        /// Genesis-configured cap (`max_builder_bps`).
        cap: u16,
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

    validate_attestation_config(&cfg.attestation)?;

    Ok(())
}

/// Per-attestation-module invariants. Mirrors the runtime checks
/// that the `RegisterAttestorSet` and `RegisterSchema` handlers run
/// at submission time, applied at genesis-load time so a
/// misconfigured `AttestationConfig` fails fast instead of writing
/// state that later handlers reject.
///
/// Tracking issue: ligate-io/ligate-chain#175.
fn validate_attestation_config<S: Spec>(
    cfg: &attestation::AttestationConfig<S>,
) -> Result<(), GenesisError> {
    use std::collections::HashMap;

    // Pass 1: validate each `InitialAttestorSet` and collect derived
    // ids for the dedup + cross-reference checks below.
    let mut set_first_seen: HashMap<attestation::AttestorSetId, usize> = HashMap::new();
    for (index, set) in cfg.initial_attestor_sets.iter().enumerate() {
        if set.members.is_empty() {
            return Err(GenesisError::InitialAttestorSetEmpty { index });
        }
        let count = set.members.len();
        if set.threshold == 0 || usize::from(set.threshold) > count {
            return Err(GenesisError::InitialAttestorSetInvalidThreshold {
                index,
                threshold: set.threshold,
                count,
            });
        }

        // The runtime sorts members on registration; the derived id
        // collapses any caller-supplied ordering. Mirror that here so
        // genesis dedup catches "same set written in two orders".
        let mut sorted = set.members.clone();
        sorted.sort_unstable_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
        let id = attestation::AttestorSet::derive_id(&sorted, set.threshold);

        if let Some(&first) = set_first_seen.get(&id) {
            return Err(GenesisError::InitialAttestorSetDuplicate { first, index });
        }
        set_first_seen.insert(id, index);
    }

    // Pass 2: validate each `InitialSchema` against the cap, against
    // the orphan-routing rule, against the set we just collected,
    // and dedup against earlier schema entries.
    let cap = cfg.max_builder_bps;
    let mut schema_first_seen: HashMap<attestation::SchemaId, usize> = HashMap::new();
    for (index, schema) in cfg.initial_schemas.iter().enumerate() {
        if schema.fee_routing_bps > cap {
            return Err(GenesisError::InitialSchemaFeeRoutingExceedsCap {
                index,
                name: schema.name.clone(),
                bps: schema.fee_routing_bps,
                cap,
            });
        }
        let has_addr = schema.fee_routing_addr.is_some();
        if (schema.fee_routing_bps > 0) != has_addr {
            return Err(GenesisError::InitialSchemaOrphanRouting {
                index,
                name: schema.name.clone(),
                bps: schema.fee_routing_bps,
                has_addr,
            });
        }
        if !set_first_seen.contains_key(&schema.attestor_set) {
            return Err(GenesisError::InitialSchemaUnknownAttestorSet {
                index,
                name: schema.name.clone(),
                attestor_set: schema.attestor_set.to_string(),
            });
        }

        let id = attestation::Schema::<S>::derive_id(&schema.owner, &schema.name, schema.version);
        if let Some(&first) = schema_first_seen.get(&id) {
            return Err(GenesisError::InitialSchemaDuplicate { first, index });
        }
        schema_first_seen.insert(id, index);
    }

    Ok(())
}

fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T, GenesisError> {
    let bytes =
        fs::read(path).map_err(|source| GenesisError::Io { path: path.to_path_buf(), source })?;
    serde_json::from_slice(&bytes)
        .map_err(|source| GenesisError::ParseJson { path: path.to_path_buf(), source })
}
