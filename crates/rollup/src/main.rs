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
use std::time::Duration;

use anyhow::Context as _;
use clap::Parser;
use ligate_rollup::{chain_config, metrics, CelestiaLigateRollup, MockLigateRollup};
use ligate_stf::genesis_config::GenesisPaths;
use sov_address::MultiAddressEvm;
use sov_celestia_adapter::CelestiaService;
use sov_mock_da::storable::StorableMockDaService;
use sov_modules_api::execution_mode::Native;
use sov_modules_rollup_blueprint::logging::initialize_logging;
use sov_modules_rollup_blueprint::FullNodeBlueprint;
use sov_stf_runner::processes::RollupProverConfig;
use sov_stf_runner::RollupConfig;
use tokio::signal::unix::{signal, SignalKind};
use tracing::debug;

/// Hard upper bound on shutdown duration after the first SIGTERM/SIGINT/SIGQUIT.
/// If the process is still alive after this many seconds, the watchdog forces
/// `std::process::exit(0)` regardless of whether tasks have finished cleaning up.
///
/// Sized so that:
/// - the SDK's HTTP server timeouts (20s for axum keepalive drain + 5s for jsonrpsee
///   `stopped()`) get the chance to complete naturally;
/// - the chain still always dies well below systemd's default `TimeoutStopSec=5min`,
///   keeping operator restart cycles fast (single-digit seconds in the common case,
///   tens of seconds in the worst case);
/// - a Replica winning the Postgres lock via the SDK's "exit to restart as leader"
///   path can never wedge the whole cluster (chain#435 incident, 2026-05-20).
///
/// Background filed at <https://github.com/ligate-io/ligate-chain/issues/435>.
const SHUTDOWN_WATCHDOG_DEADLINE: Duration = Duration::from_secs(35);

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

    // Belt-and-suspenders shutdown watchdog. The SDK installs its own SIGTERM /
    // SIGINT / SIGQUIT handlers that drive a graceful shutdown via `watch::Sender`
    // and a per-task cancellation cascade. This task does NOT replace that;
    // it complements it by enforcing a hard upper bound on the total shutdown
    // duration. If any task on the cancellation graph stalls (an HTTP server
    // waiting on a keepalive connection that never closes, a Postgres pool with
    // a hung connection, a future regression to the SDK's task tree, etc.), this
    // watchdog calls `std::process::exit(0)` rather than letting systemd's
    // `TimeoutStopSec` (5 minutes by default) elapse and SIGKILL the process.
    //
    // See ligate-io/ligate-chain#435 for the incident this was added for.
    spawn_shutdown_watchdog();

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

/// Arms the shutdown watchdog. See the call site in `main()` for context.
fn spawn_shutdown_watchdog() {
    tokio::spawn(async move {
        // Listen for any of the standard "please stop" signals. We watch all three
        // so the deadline applies regardless of how shutdown was triggered (systemd
        // sends SIGTERM, a manual `kill` defaults to SIGTERM, Ctrl-C sends SIGINT,
        // `kill -3` sends SIGQUIT). The first one to arrive starts the deadline
        // clock.
        let mut sigterm = match signal(SignalKind::terminate()) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(error = %e, "watchdog: failed to install SIGTERM listener; the SDK's own handler is still in effect");
                return;
            }
        };
        let mut sigint = match signal(SignalKind::interrupt()) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(error = %e, "watchdog: failed to install SIGINT listener; the SDK's own handler is still in effect");
                return;
            }
        };
        let mut sigquit = match signal(SignalKind::quit()) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(error = %e, "watchdog: failed to install SIGQUIT listener; the SDK's own handler is still in effect");
                return;
            }
        };

        tokio::select! {
            _ = sigterm.recv() => tracing::info!(
                deadline_secs = SHUTDOWN_WATCHDOG_DEADLINE.as_secs(),
                "watchdog: SIGTERM received; arming hard-exit deadline"
            ),
            _ = sigint.recv() => tracing::info!(
                deadline_secs = SHUTDOWN_WATCHDOG_DEADLINE.as_secs(),
                "watchdog: SIGINT received; arming hard-exit deadline"
            ),
            _ = sigquit.recv() => tracing::info!(
                deadline_secs = SHUTDOWN_WATCHDOG_DEADLINE.as_secs(),
                "watchdog: SIGQUIT received; arming hard-exit deadline"
            ),
        }

        tokio::time::sleep(SHUTDOWN_WATCHDOG_DEADLINE).await;
        tracing::error!(
            deadline_secs = SHUTDOWN_WATCHDOG_DEADLINE.as_secs(),
            "watchdog: graceful shutdown did not complete within the deadline; \
             forcing process exit. This indicates a stalled task somewhere in the \
             cancellation graph; check logs for the most recent non-shutdown lines."
        );
        std::process::exit(0);
    });
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
    // #181: pull `[chain]` out of the rollup TOML before handing the
    // rest to the SDK loader. The SDK's `RollupConfig` rejects
    // unknown fields, so the two sections can't share the same
    // typed deserialization.
    let (chain, residual_toml) = chain_config::load_split_config(rollup_config_path)
        .with_context(|| format!("Failed to load chain config from {rollup_config_path}"))?;
    let rollup_config: RollupConfig<MultiAddressEvm, StorableMockDaService> =
        toml::from_str(&residual_toml).with_context(|| {
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

    metrics::spawn_state_db_size_task(
        rollup_config.storage.path.clone(),
        metrics::DEFAULT_STATE_DB_SIZE_POLL_INTERVAL,
    );

    let rollup = MockLigateRollup::<Native>::new(chain.chain_id)
        .create_new_rollup(
            genesis_paths,
            rollup_config,
            RollupProverConfig::Disabled,
            None,  // start_at_rollup_height
            None,  // stop_at_rollup_height
            None,  // exec_config (we use `()`)
            false, // start_fresh_outer_proof_on_resync
        )
        .await
        .context("Failed to initialize Ligate rollup on mock DA")?;

    rollup.run().await
}

async fn run_with_celestia(
    rollup_config_path: &str,
    genesis_paths: &GenesisPaths,
) -> anyhow::Result<()> {
    // #181: same two-pass split as the mock path. `[chain]` lives in
    // the rollup TOML alongside `[da]`, `[storage]`, etc.
    let (chain, residual_toml) = chain_config::load_split_config(rollup_config_path)
        .with_context(|| format!("Failed to load chain config from {rollup_config_path}"))?;
    let rollup_config: RollupConfig<MultiAddressEvm, CelestiaService> =
        toml::from_str(&residual_toml).with_context(|| {
            format!("Failed to read rollup configuration from {rollup_config_path}")
        })?;

    std::fs::create_dir_all(&rollup_config.storage.path).with_context(|| {
        format!(
            "Failed to create rollup storage directory at {}",
            rollup_config.storage.path.display()
        )
    })?;

    metrics::spawn_state_db_size_task(
        rollup_config.storage.path.clone(),
        metrics::DEFAULT_STATE_DB_SIZE_POLL_INTERVAL,
    );

    let rollup = CelestiaLigateRollup::<Native>::new(chain.chain_id)
        .create_new_rollup(
            genesis_paths,
            rollup_config,
            RollupProverConfig::Disabled,
            None,  // start_at_rollup_height
            None,  // stop_at_rollup_height
            None,  // exec_config
            false, // start_fresh_outer_proof_on_resync
        )
        .await
        .context("Failed to initialize Ligate rollup on Celestia DA")?;

    rollup.run().await
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    /// `--mode` was removed in chain#446 along with `NodeRole::Follower`.
    /// Guard against an inadvertent re-introduction that breaks operator
    /// scripts which expect `--mode` to be unrecognised.
    #[test]
    fn mode_arg_is_rejected() {
        let result = Args::try_parse_from(["ligate-node", "--mode", "sequencer"]);
        assert!(result.is_err());
    }

    /// `--da-layer` default is still `mock` (the local-dev experience).
    #[test]
    fn da_layer_defaults_to_mock() {
        let args = Args::parse_from(["ligate-node"]);
        assert_eq!(args.da_layer, SupportedDaLayer::Mock);
    }
}
