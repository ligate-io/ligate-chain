//! Ligate Chain rollup binary.
//!
//! Thin CLI dispatcher around the blueprint impls in
//! [`ligate_rollup`].
//!
//! ## v0 scope
//!
//! - DA layer: `mock` (in-process SQLite-backed) or `celestia` (real
//!   Mocha testnet / local light node).
//! - zkVM: mock only. Real proving is Phase A.4.
//! - Prover mode: hard-coded to `Disabled`. Mock proving exists but
//!   serves no purpose with mock zkVM, and we want startup to be
//!   fast for devnet smoke testing.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::exit;

use anyhow::Context as _;
use clap::Parser;
use ligate_rollup::{metrics, CelestiaLigateRollup, MockLigateRollup};
use ligate_stf::genesis_config::GenesisPaths;
use sov_address::MultiAddressEvm;
use sov_celestia_adapter::CelestiaService;
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
    /// Which DA layer to run against.
    ///
    /// `mock` uses an in-process SQLite-backed mock DA — the default
    /// devnet experience. `celestia` connects to a Celestia light
    /// node (Mocha testnet or local) using the credentials in the
    /// rollup config TOML's `[da]` section.
    #[arg(long, default_value = "mock", value_enum)]
    da_layer: SupportedDaLayer,

    /// Path to the rollup config TOML.
    ///
    /// Format mirrors the SDK's `RollupConfig` (storage, sequencer,
    /// proof-manager sections). The `[da]` section's shape depends on
    /// `--da-layer`. Defaults pick the matching `devnet/*.toml` file
    /// for the chosen DA layer.
    #[arg(long, default_value = None)]
    rollup_config_path: Option<String>,

    /// Directory containing per-module genesis JSON files
    /// (`bank.json`, `accounts.json`, …). See
    /// [`ligate_stf::genesis_config`] for the expected layout.
    #[arg(long, default_value = "devnet/genesis")]
    genesis_config_dir: PathBuf,

    /// Address to bind the Prometheus `/metrics` endpoint on.
    ///
    /// Defaults to `127.0.0.1:9100` so a fresh `cargo run --bin
    /// ligate-node` exposes metrics for local scraping without
    /// leaking them to the network. Operators behind a reverse
    /// proxy override to a public bind. Pass `off` to disable the
    /// endpoint entirely.
    ///
    /// Tracking issue: #110.
    #[arg(long, default_value = "127.0.0.1:9100")]
    metrics_bind: String,
}

/// DA layers we know how to dispatch into.
#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
enum SupportedDaLayer {
    /// In-process mock DA. Single-node, no external dependencies.
    Mock,
    /// Real Celestia DA. Requires a reachable light/bridge node.
    Celestia,
}

impl SupportedDaLayer {
    /// Default rollup-config path for this DA layer when the user
    /// doesn't pass `--rollup-config-path`.
    fn default_rollup_config_path(self) -> &'static str {
        match self {
            SupportedDaLayer::Mock => "devnet/rollup.toml",
            SupportedDaLayer::Celestia => "devnet/celestia.toml",
        }
    }
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
    // before any TLS work happens (jsonrpsee handshakes, Celestia
    // gRPC, etc.). The SDK uses `ring`; we match.
    rustls::crypto::ring::default_provider()
        .install_default()
        .map_err(|e| anyhow::anyhow!("Failed to install rustls ring crypto provider: {e:?}"))?;

    let rollup_config_path = args
        .rollup_config_path
        .clone()
        .unwrap_or_else(|| args.da_layer.default_rollup_config_path().to_string());

    debug!(
        da_layer = ?args.da_layer,
        rollup_config_path = %rollup_config_path,
        genesis_config_dir = ?args.genesis_config_dir,
        metrics_bind = %args.metrics_bind,
        "Starting Ligate rollup",
    );

    // Spawn the Prometheus `/metrics` server before the rollup
    // takes the foreground tokio task. If the user passed
    // `--metrics-bind off`, skip the spawn and run blind. We do
    // NOT join the metrics task back into the main flow; on
    // rollup shutdown the task gets dropped with the runtime.
    if !is_metrics_disabled(&args.metrics_bind) {
        let addr: SocketAddr = args
            .metrics_bind
            .parse()
            .with_context(|| format!("invalid --metrics-bind value: {}", args.metrics_bind))?;
        // Pre-touch the attestation counters so they appear in
        // `/metrics` output even before the first tx lands.
        attestation::metrics::init();
        let listener = metrics::bind(addr)
            .await
            .with_context(|| format!("metrics server failed to bind {addr}"))?;
        tokio::spawn(async move {
            if let Err(e) = metrics::serve(listener).await {
                tracing::error!(error = ?e, "metrics server stopped");
            }
        });
    }

    let genesis_paths = GenesisPaths::from_dir(&args.genesis_config_dir);

    match args.da_layer {
        SupportedDaLayer::Mock => run_with_mock(&rollup_config_path, &genesis_paths).await,
        SupportedDaLayer::Celestia => run_with_celestia(&rollup_config_path, &genesis_paths).await,
    }
}

/// `--metrics-bind off` (case-insensitive) disables the metrics
/// endpoint entirely. Any other value gets parsed as a `SocketAddr`.
fn is_metrics_disabled(value: &str) -> bool {
    matches!(value.to_ascii_lowercase().as_str(), "off" | "none" | "disabled")
}

async fn run_with_mock(
    rollup_config_path: &str,
    genesis_paths: &GenesisPaths,
) -> anyhow::Result<()> {
    let rollup_config: RollupConfig<MultiAddressEvm, StorableMockDaService> =
        from_toml_path(rollup_config_path).with_context(|| {
            format!("Failed to read rollup configuration from {rollup_config_path}")
        })?;

    // SQLite (mock DA) + RocksDB (state) both refuse to create
    // files if the parent directory is missing, and we want
    // first-run `cargo run --bin ligate-node` to work without a
    // separate `mkdir`. Celestia doesn't use the storage path for
    // DA blobs (those go on the actual DA layer) but RocksDB
    // still does, so the create_dir_all is shared.
    std::fs::create_dir_all(&rollup_config.storage.path).with_context(|| {
        format!(
            "Failed to create rollup storage directory at {}",
            rollup_config.storage.path.display()
        )
    })?;

    let rollup = MockLigateRollup::<Native>::default()
        .create_new_rollup(
            genesis_paths,
            rollup_config,
            RollupProverConfig::Disabled,
            None, // start_at_rollup_height
            None, // stop_at_rollup_height
            None, // exec_config (we use `()`)
        )
        .await
        .context("Failed to initialize Ligate rollup on mock DA")?;

    rollup.run().await
}

async fn run_with_celestia(
    rollup_config_path: &str,
    genesis_paths: &GenesisPaths,
) -> anyhow::Result<()> {
    let rollup_config: RollupConfig<MultiAddressEvm, CelestiaService> =
        from_toml_path(rollup_config_path).with_context(|| {
            format!("Failed to read rollup configuration from {rollup_config_path}")
        })?;

    std::fs::create_dir_all(&rollup_config.storage.path).with_context(|| {
        format!(
            "Failed to create rollup storage directory at {}",
            rollup_config.storage.path.display()
        )
    })?;

    let rollup = CelestiaLigateRollup::<Native>::default()
        .create_new_rollup(
            genesis_paths,
            rollup_config,
            RollupProverConfig::Disabled,
            None,
            None,
            None,
        )
        .await
        .context("Failed to initialize Ligate rollup on Celestia DA")?;

    rollup.run().await
}
