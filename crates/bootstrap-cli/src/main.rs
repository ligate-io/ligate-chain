//! `ligate-bootstrap` — operator post-genesis bootstrap.
//!
//! Run once after a fresh devnet boots to register the canonical
//! schemas (`themisra.proof-of-prompt/v1`, future first-party schemas)
//! and their attestor sets.
//!
//! ## Why a separate binary
//!
//! The chain ships with `initial_attestor_sets: []` and
//! `initial_schemas: []` at genesis (see `devnet-1/genesis/attestation.json`).
//! Schema registration happens via on-chain transactions, the same
//! way any third-party schema would register. Ligate Labs registers
//! `themisra.proof-of-prompt` exactly the way Themisra documents
//! every other partner doing it — keeping the chain's permissionless
//! design promise honest.
//!
//! ## Usage
//!
//! ```sh
//! # Compute the payload_shape_hash for a schema doc (no network):
//! ligate-bootstrap compute-shape-hash docs/protocol/schemas/themisra.proof-of-prompt-v1.json
//!
//! # Run the full ceremony against a running node:
//! ligate-bootstrap register-canonical-schemas \
//!     --config devnet-1/canonical-schemas.toml \
//!     --signer-key /etc/ligate/keys/themisra-bootstrap.key \
//!     --rpc http://localhost:12346
//! ```
//!
//! Idempotent: re-runs skip already-registered ids.
//!
//! ## Tracking
//!
//! - Path B of the post-genesis schema-registration decision
//!   ([discussion](https://github.com/ligate-io/ligate-chain/issues/231)).
//! - LIP-5 (#185) iterates the `proof-of-prompt/v1` spec; this binary
//!   stays the registration tool of record across spec revisions.

mod config;
mod register;
mod shape_hash;

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use crate::config::Config;
use crate::register::{RegisterParams, SchemaOutcome, TxOutcome};

#[derive(Parser, Debug)]
#[command(
    name = "ligate-bootstrap",
    version,
    about = "Post-genesis bootstrap: register canonical Ligate schemas",
    long_about = None,
)]
struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Compute the `payload_shape_hash` for a JSON Schema doc on disk.
    /// Doesn't touch the network; pure I/O + SHA-256.
    ComputeShapeHash {
        /// Path to the JSON Schema file.
        path: PathBuf,
    },

    /// Submit `RegisterAttestorSet` + `RegisterSchema` for each entry
    /// in a `canonical-schemas.toml`. Idempotent.
    RegisterCanonicalSchemas {
        /// Path to `canonical-schemas.toml`.
        #[arg(long, value_name = "FILE")]
        config: PathBuf,
        /// Path to the operator's signing key file (32-byte hex,
        /// matching `ligate-genesis-tool keys generate`'s `<role>.key`
        /// output).
        #[arg(long, value_name = "FILE")]
        signer_key: PathBuf,
        /// Chain RPC URL. The `/v1` suffix is auto-appended.
        #[arg(long, value_name = "URL", default_value = "http://localhost:12346")]
        rpc: String,
        /// Override `chain_id`. Default: fetched from `GET /v1/rollup/info`.
        /// Pass an explicit value if the node hasn't started serving
        /// `/rollup/info` yet (rare, but possible during early boot).
        #[arg(long)]
        chain_id: Option<u64>,
        /// Override `chain_hash` (64-char hex). Default: fetched from
        /// `GET /v1/rollup/info`.
        #[arg(long)]
        chain_hash: Option<String>,
        /// Per-tx max fee in nano-AVOW. Default: 200_000_000 (0.2 AVOW).
        #[arg(long)]
        max_fee_nano: Option<u128>,
    },
}

fn main() -> ExitCode {
    let args = Args::parse();
    let runtime =
        tokio::runtime::Builder::new_current_thread().enable_all().build().expect("tokio runtime");
    let result = runtime.block_on(run(args));
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

async fn run(args: Args) -> Result<()> {
    match args.command {
        Command::ComputeShapeHash { path } => {
            let hash = shape_hash::compute(&path)?;
            println!("{}", hex::encode(hash));
            Ok(())
        }
        Command::RegisterCanonicalSchemas {
            config,
            signer_key,
            rpc,
            chain_id,
            chain_hash,
            max_fee_nano,
        } => {
            let cfg = Config::load(&config)?;
            let signer = load_signer_key(&signer_key)?;

            // Resolve chain identity: explicit overrides if both present;
            // otherwise fetch what we need from /rollup/info.
            let (chain_id_val, chain_hash_val) =
                if let (Some(id), Some(h)) = (chain_id, chain_hash.clone()) {
                    (id, parse_chain_hash(&h)?)
                } else {
                    let info = register::fetch_rollup_info(&rpc).await?;
                    eprintln!("connected to {} (ligate-node {})", info.chain_id, info.version);
                    let id = chain_id.unwrap_or_else(|| {
                        // `chain_id` in /rollup/info is the human-readable
                        // string ("ligate-devnet-2"); the chain signs txs
                        // against a u64 numeric id defined as `CHAIN_ID`
                        // in `constants.toml`. Until /rollup/info also
                        // exposes the numeric form, operators must pass
                        // --chain-id explicitly. Default to 0 with a loud
                        // warning so a typo'd run fails-fast at submit.
                        eprintln!(
                            "warning: --chain-id not set, and /rollup/info returned only \
                             the string id `{}`. Pass --chain-id <u64> matching \
                             CHAIN_ID in constants.toml. Defaulting to 0; submit will likely fail.",
                            info.chain_id
                        );
                        0
                    });
                    let h_bytes = match chain_hash {
                        Some(h) => parse_chain_hash(&h)?,
                        None => parse_chain_hash(&info.chain_hash)?,
                    };
                    (id, h_bytes)
                };

            let params = RegisterParams {
                config_path: &config,
                config: cfg,
                signer_key: signer,
                chain_id: chain_id_val,
                chain_hash: chain_hash_val,
                rpc_url: rpc,
                max_fee_nano,
            };
            let outcomes = register::run(params).await?;
            print_outcomes(&outcomes);
            Ok(())
        }
    }
}

fn load_signer_key(
    path: &std::path::Path,
) -> Result<<<ligate_rollup::MockRollupSpec<sov_modules_api::execution_mode::Native> as sov_modules_api::Spec>::CryptoSpec as sov_modules_api::CryptoSpec>::PrivateKey>{
    let raw =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    // `ligate-genesis-tool keys generate` writes the key as a single
    // hex line terminated with `\n`. Strip whitespace; tolerate
    // optional `0x` prefix.
    let trimmed = raw.trim();
    let hex = trimmed.strip_prefix("0x").unwrap_or(trimmed);
    let bytes = hex::decode(hex)
        .with_context(|| format!("hex-decoding signer key from {}", path.display()))?;
    use sov_modules_api::CryptoSpec;
    type S = ligate_rollup::MockRollupSpec<sov_modules_api::execution_mode::Native>;
    type Sk = <<S as sov_modules_api::Spec>::CryptoSpec as CryptoSpec>::PrivateKey;
    Sk::try_from(bytes).map_err(|e| anyhow::anyhow!("loading signer key: {e:?}"))
}

fn parse_chain_hash(s: &str) -> Result<[u8; 32]> {
    let trimmed = s.trim();
    let hex = trimmed.strip_prefix("0x").unwrap_or(trimmed);
    let bytes = hex::decode(hex).with_context(|| format!("hex-decoding chain_hash {s:?}"))?;
    if bytes.len() != 32 {
        anyhow::bail!("chain_hash must be 32 bytes, got {}", bytes.len());
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes);
    Ok(out)
}

fn print_outcomes(outcomes: &[SchemaOutcome]) {
    println!();
    println!("Canonical schema registration: {} schema(s)", outcomes.len());
    for o in outcomes {
        println!();
        println!("  {} v{}", o.name, o.version);
        println!("    attestor_set_id    : {}", o.attestor_set_id);
        println!("      → {}", outcome_label(o.attestor_set_status));
        println!("    schema_id          : {}", o.schema_id);
        println!("      → {}", outcome_label(o.schema_status));
        println!("    payload_shape_hash : {}", hex::encode(o.payload_shape_hash));
    }
    println!();
}

fn outcome_label(o: TxOutcome) -> &'static str {
    match o {
        TxOutcome::Submitted => "submitted (new)",
        TxOutcome::AlreadyRegistered => "already registered (skipped)",
    }
}
