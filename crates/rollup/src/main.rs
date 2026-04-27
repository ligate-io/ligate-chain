//! Ligate Chain rollup binary.
//!
//! Thin CLI dispatcher around [`ligate_rollup::MockLigateRollup`].
//!
//! ## v0 scope
//!
//! - DA layer: mock only. Celestia is Phase A.3.
//! - zkVM: mock only. Real proving is Phase A.4.
//! - Prover mode: hard-coded to `Disabled`. Mock proving exists but
//!   serves no purpose with mock zkVM, and we want startup to be
//!   fast for devnet smoke testing.
//!
//! The CLI is shaped so adding a `--da-layer` flag in Phase A.3 is
//! a non-breaking change: today there's only one combination so we
//! don't expose the flag yet.

use std::path::PathBuf;
use std::process::exit;

use anyhow::Context as _;
use clap::Parser;
use ligate_rollup::MockLigateRollup;
use ligate_stf::genesis_config::GenesisPaths;
use sov_address::MultiAddressEvm;
use sov_mock_da::storable::StorableMockDaService;
use sov_modules_api::execution_mode::Native;
use sov_modules_rollup_blueprint::logging::initialize_logging;
use sov_modules_rollup_blueprint::FullNodeBlueprint;
use sov_stf_runner::processes::RollupProverConfig;
use sov_stf_runner::{from_toml_path, RollupConfig};
use tracing::debug;

#[derive(Parser, Debug)]
#[command(author, version, about = "Ligate Chain rollup node", long_about = None)]
struct Args {
    /// Path to the rollup config TOML.
    ///
    /// Format mirrors the SDK's `RollupConfig` (storage, sequencer,
    /// proof-manager sections). A devnet-ready example will land in
    /// `devnet/rollup.toml` in Phase A.2.3.
    #[arg(long, default_value = "devnet/rollup.toml")]
    rollup_config_path: String,

    /// Directory containing per-module genesis JSON files (`bank.json`,
    /// `accounts.json`, …). See [`ligate_stf::genesis_config`] for the
    /// expected layout.
    #[arg(long, default_value = "devnet/genesis")]
    genesis_config_dir: PathBuf,
}

#[tokio::main]
async fn main() {
    // The opentelemetry exporter wants this guard alive for the
    // whole process; dropping it stalls log flush on shutdown.
    let _guard = initialize_logging();

    match run().await {
        Ok(()) => {
            tracing::debug!("Rollup execution complete. Shutting down.");
        }
        Err(e) => {
            tracing::error!(
                error = ?e,
                backtrace = e.backtrace().to_string(),
                "Rollup execution failed",
            );
            exit(1);
        }
    }
}

async fn run() -> anyhow::Result<()> {
    let args = Args::parse();

    // `rustls` requires a default crypto provider be installed
    // before any TLS work happens (jsonrpsee handshakes, etc.). The
    // SDK uses `ring`; we match.
    rustls::crypto::ring::default_provider()
        .install_default()
        .map_err(|e| anyhow::anyhow!("Failed to install rustls ring crypto provider: {e:?}"))?;

    debug!(
        rollup_config_path = %args.rollup_config_path,
        genesis_config_dir = ?args.genesis_config_dir,
        "Starting Ligate rollup on mock DA",
    );

    let rollup_config: RollupConfig<MultiAddressEvm, StorableMockDaService> =
        from_toml_path(&args.rollup_config_path).with_context(|| {
            format!("Failed to read rollup configuration from {}", args.rollup_config_path)
        })?;

    // Make sure the storage path exists before any SDK component
    // reaches into it. SQLite (mock DA) and RocksDB (state) both
    // refuse to create their files if the parent directory is
    // missing, and we want first-run `cargo run --bin ligate-node`
    // to work without a separate `mkdir` step.
    std::fs::create_dir_all(&rollup_config.storage.path).with_context(|| {
        format!(
            "Failed to create rollup storage directory at {}",
            rollup_config.storage.path.display()
        )
    })?;

    let genesis_paths = GenesisPaths::from_dir(&args.genesis_config_dir);

    let rollup = MockLigateRollup::<Native>::default()
        .create_new_rollup(
            &genesis_paths,
            rollup_config,
            // Disabled until Phase A.4. With MockZkvm enabling
            // proving costs cycles for no security gain.
            RollupProverConfig::Disabled,
            None, // start_at_rollup_height
            None, // stop_at_rollup_height
            None, // exec_config (we use `()`)
        )
        .await
        .context("Failed to initialize Ligate rollup on mock DA")?;

    rollup.run().await
}
