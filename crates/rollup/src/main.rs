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

    /// Operator role this node should run as.
    ///
    /// `sequencer` (default) runs the normal full-node-with-sequencer
    /// flow: builds batches from incoming transactions and posts them
    /// to the DA layer. Use this on the official Ligate Labs node and
    /// any other node whose DA address is registered in
    /// `sequencer_registry.json` and that holds the corresponding
    /// signer key with funded TIA.
    ///
    /// `follower` runs the same binary but disables automatic batch
    /// production. The node still syncs the STF from DA, serves all
    /// read-side REST endpoints, and exposes `/health` and
    /// `/metrics`. This is the right mode for design partners,
    /// auditors, and anyone running their own Ligate node who isn't
    /// the registered sequencer. Submission to a follower's
    /// `POST /v1/sequencer/txs` is currently still accepted but will
    /// not propagate (its blob would be filtered at the STF level
    /// because the DA address isn't in the registry); point clients
    /// at the upstream sequencer (`https://rpc.ligate.io` for public
    /// devnet) for submission.
    ///
    /// Tracking: #243 (this flag), #82 (multi-sequencer for v1+).
    #[arg(long, default_value = "sequencer", value_enum)]
    mode: NodeRole,

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

/// Role this node operates as. See `Args::mode` for the operator-
/// facing semantics. Mirrors [`ligate_rollup::NodeRole`] but with
/// `clap::ValueEnum` derived for CLI parsing; the lib's variant
/// stays clap-free so the library has no clap dep.
#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
enum NodeRole {
    /// Normal full node: produces batches, posts to DA, accepts
    /// submissions on `/v1/sequencer/txs`.
    Sequencer,
    /// Read-only follower: syncs STF from DA, serves read endpoints,
    /// does not auto-produce batches, returns 503 on submissions.
    Follower,
}

impl From<NodeRole> for ligate_rollup::NodeRole {
    fn from(role: NodeRole) -> Self {
        match role {
            NodeRole::Sequencer => ligate_rollup::NodeRole::Sequencer,
            NodeRole::Follower => ligate_rollup::NodeRole::Follower,
        }
    }
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
        mode = ?args.mode,
        rollup_config_path = %rollup_config_path,
        genesis_config_dir = ?args.genesis_config_dir,
        metrics_bind = %args.metrics_bind,
        "Starting Ligate rollup",
    );

    // Surface follower mode at INFO so it shows up in default-level
    // operator logs. Followers behave very differently from
    // sequencers in subtle ways (no batch production, submissions
    // don't propagate); making this visible at startup avoids the
    // "why aren't my transactions landing?" footgun.
    if args.mode == NodeRole::Follower {
        tracing::info!(
            "ligate-node booting in FOLLOWER mode: this node will sync the STF \
             from DA but will NOT produce batches. Submissions to this node's \
             /v1/sequencer/txs endpoint return 503 Service Unavailable. Point \
             clients at the upstream sequencer (e.g. https://rpc.ligate.io) for \
             transaction submission."
        );
    }

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
        SupportedDaLayer::Mock => {
            run_with_mock(&rollup_config_path, &genesis_paths, args.mode).await
        }
        SupportedDaLayer::Celestia => {
            run_with_celestia(&rollup_config_path, &genesis_paths, args.mode).await
        }
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
    mode: NodeRole,
) -> anyhow::Result<()> {
    // #181: pull `[chain]` out of the rollup TOML before handing the
    // rest to the SDK loader. The SDK's `RollupConfig` rejects
    // unknown fields, so the two sections can't share the same
    // typed deserialization.
    let (chain, residual_toml) = chain_config::load_split_config(rollup_config_path)
        .with_context(|| format!("Failed to load chain config from {rollup_config_path}"))?;
    let mut rollup_config: RollupConfig<MultiAddressEvm, StorableMockDaService> =
        toml::from_str(&residual_toml).with_context(|| {
            format!("Failed to read rollup configuration from {rollup_config_path}")
        })?;

    // #243: in follower mode, disable automatic batch production. The
    // sequencer machinery still instantiates (the SDK's blueprint
    // requires a sequencer instance for `api_state` plumbing), but it
    // doesn't post batches to DA, so no spam-failed blob submissions.
    // The submission REST endpoint stays mounted; a true 503 there
    // is a follow-up requiring an SDK override.
    if mode == NodeRole::Follower {
        rollup_config.sequencer.automatic_batch_production = false;
    }

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

    let rollup = MockLigateRollup::<Native>::new_with_role(chain.chain_id, mode.into())
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
    mode: NodeRole,
) -> anyhow::Result<()> {
    // #181: same two-pass split as the mock path. `[chain]` lives in
    // the rollup TOML alongside `[da]`, `[storage]`, etc.
    let (chain, residual_toml) = chain_config::load_split_config(rollup_config_path)
        .with_context(|| format!("Failed to load chain config from {rollup_config_path}"))?;
    let mut rollup_config: RollupConfig<MultiAddressEvm, CelestiaService> =
        toml::from_str(&residual_toml).with_context(|| {
            format!("Failed to read rollup configuration from {rollup_config_path}")
        })?;

    // #243: see equivalent comment in `run_with_mock`. In follower
    // mode we disable auto batch production so a follower never tries
    // to post blobs to Celestia (which would burn TIA and fail at the
    // STF level if the DA address isn't in the registry).
    if mode == NodeRole::Follower {
        rollup_config.sequencer.automatic_batch_production = false;
    }

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

    let rollup = CelestiaLigateRollup::<Native>::new_with_role(chain.chain_id, mode.into())
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

    /// Default `--mode` when omitted should be `sequencer` so existing
    /// invocations of `cargo run --bin ligate-node` keep working
    /// without operator action.
    #[test]
    fn mode_defaults_to_sequencer() {
        let args = Args::parse_from(["ligate-node"]);
        assert_eq!(args.mode, NodeRole::Sequencer);
    }

    #[test]
    fn mode_parses_follower() {
        let args = Args::parse_from(["ligate-node", "--mode", "follower"]);
        assert_eq!(args.mode, NodeRole::Follower);
    }

    #[test]
    fn mode_parses_explicit_sequencer() {
        let args = Args::parse_from(["ligate-node", "--mode", "sequencer"]);
        assert_eq!(args.mode, NodeRole::Sequencer);
    }

    /// Spelling counts: clap's `value_enum` produces `kebab-case`
    /// matching the variant names. Guard against an inadvertent
    /// rename (e.g. `replica`) breaking external operator scripts.
    #[test]
    fn mode_rejects_unknown_value() {
        let result = Args::try_parse_from(["ligate-node", "--mode", "replica"]);
        assert!(result.is_err());
    }
}
