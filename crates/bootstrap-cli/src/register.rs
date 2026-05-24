//! `register-canonical-schemas` ceremony.
//!
//! Reads a `canonical-schemas.toml`, hashes each referenced JSON
//! Schema doc, and submits `RegisterAttestorSet` + `RegisterSchema`
//! txs to the chain. Idempotent: queries the chain for each id first
//! and skips already-registered ones, so the script is safe to re-run
//! after a SIGINT or partial failure.
//!
//! Wire-format gotchas it hides:
//! - The chain's `POST /v1/sequencer/txs` accepts the inner signed-tx
//!   bytes; we do NOT pre-wrap in `AuthenticatorInput::Standard`. (See
//!   [`ligate-chain#245`](https://github.com/ligate-io/ligate-chain/issues/245).)
//! - Inclusion polling is HTTP, not WebSocket. ([`ligate-cli#8`](https://github.com/ligate-io/ligate-cli/issues/8).)
//! - The `/v1` path prefix is auto-applied to whatever RPC URL the
//!   operator passes (cli #7 / faucet #5 mirror this same idempotent
//!   `/v1` append).

use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use attestation::{
    AttestorSet, AttestorSetId, CallMessage as AttestationCall, PubKey, Schema, SchemaId,
    MAX_ATTESTOR_SET_MEMBERS,
};
use ligate_client::submit::Submitter;
use ligate_rollup::MockRollupSpec;
use ligate_stf::runtime::RuntimeCall;
use serde::Deserialize;
use sov_modules_api::execution_mode::Native;
use sov_modules_api::transaction::{PriorityFeeBips, UnsignedTransaction};
use sov_modules_api::{
    capabilities::UniquenessData, Amount, CryptoSpec, PrivateKey, SafeString, SafeVec, Spec,
};

use sov_modules_api::PublicKey;

use crate::config::{Config, SchemaEntry};
use crate::shape_hash;

/// Concrete spec the binary uses end-to-end. Matches `ligate-cli`'s
/// `transfer.rs` and the chain's `RollupAuthenticator` typing, so the
/// borsh shape we sign matches what the chain re-derives at verify
/// time.
type S = MockRollupSpec<Native>;
type ChainRuntime = ligate_stf::runtime::Runtime<S>;
type SovPrivateKey = <<S as Spec>::CryptoSpec as CryptoSpec>::PrivateKey;
type SovAddress = <S as Spec>::Address;

/// Default per-tx fee envelope for registration calls (nano-AVOW).
/// Generous so a registration never fails for fee reasons under devnet
/// conditions. Operators can override via `--max-fee-nano`.
pub const DEFAULT_MAX_FEE_NANO: u128 = 200_000_000; // 0.2 AVOW

/// Inputs to [`run`]: everything needed to drive the ceremony.
pub struct RegisterParams<'a> {
    /// Path to `canonical-schemas.toml`.
    pub config_path: &'a Path,
    /// Parsed config (already validated). Pass alongside `config_path`
    /// so [`run`] can resolve `spec_file` paths relative to it.
    pub config: Config,
    /// Operator signing key (32-byte ed25519 seed).
    pub signer_key: SovPrivateKey,
    /// Chain id (`/v1/rollup/info -> chain_id`'s numeric form, NOT the
    /// `ligate-devnet-1` string).
    pub chain_id: u64,
    /// 32-byte build-time `CHAIN_HASH`, from `/v1/rollup/info`.
    pub chain_hash: [u8; 32],
    /// Chain RPC URL (with or without `/v1`; idempotent append).
    pub rpc_url: String,
    /// Max fee per tx in nano-AVOW. Defaults to [`DEFAULT_MAX_FEE_NANO`].
    pub max_fee_nano: Option<u128>,
}

/// Per-schema outcome of [`run`].
#[derive(Debug)]
pub struct SchemaOutcome {
    pub name: String,
    pub version: u32,
    pub attestor_set_id: AttestorSetId,
    pub schema_id: SchemaId,
    pub payload_shape_hash: [u8; 32],
    pub attestor_set_status: TxOutcome,
    pub schema_status: TxOutcome,
}

/// Whether each registration tx had to land or was already on-chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TxOutcome {
    /// Submitted a new tx in this run.
    Submitted,
    /// Skipped because the id already exists on-chain.
    AlreadyRegistered,
}

/// Run the ceremony end-to-end. Returns one [`SchemaOutcome`] per
/// `[[schemas]]` entry in the config.
pub async fn run(mut params: RegisterParams<'_>) -> Result<Vec<SchemaOutcome>> {
    let max_fee = params.max_fee_nano.unwrap_or(DEFAULT_MAX_FEE_NANO);
    let rpc = normalize_rpc(&params.rpc_url);

    let submitter = Submitter::new(&rpc).await.with_context(|| format!("connecting to {rpc}"))?;

    // Owner address is derived from the signer's pubkey: `pubkey[..28]`.
    // Schemas register with this owner; the schema_id depends on it.
    let signer_pubkey = params.signer_key.pub_key();
    let owner_addr: SovAddress = pubkey_to_address(&signer_pubkey);

    let schema_root = params.config.resolve_schema_root(params.config_path);

    // Pre-fetch the operator's nonce. Each subsequent submit increments
    // by 1 locally; if the chain rejects with stale-nonce, the operator
    // re-runs (the script is idempotent for already-landed registrations).
    let mut nonce = submitter
        .inner()
        .get_nonce_for_public_key::<S>(&signer_pubkey)
        .await
        .with_context(|| format!("fetching starting nonce for {owner_addr}"))?;

    let entries = std::mem::take(&mut params.config.schemas);
    let mut outcomes = Vec::with_capacity(entries.len());
    for entry in entries {
        let outcome = register_one(
            &entry,
            &schema_root,
            &owner_addr,
            &submitter,
            &rpc,
            &params.signer_key,
            params.chain_id,
            &params.chain_hash,
            &mut nonce,
            max_fee,
        )
        .await
        .with_context(|| format!("registering {} v{}", entry.name, entry.version))?;
        outcomes.push(outcome);
    }
    Ok(outcomes)
}

#[allow(clippy::too_many_arguments)]
async fn register_one(
    entry: &SchemaEntry,
    schema_root: &Path,
    owner_addr: &SovAddress,
    submitter: &Submitter,
    rpc: &str,
    signer_key: &SovPrivateKey,
    chain_id: u64,
    chain_hash: &[u8; 32],
    nonce: &mut u64,
    max_fee: u128,
) -> Result<SchemaOutcome> {
    // 1. Compute the deterministic ids from config.
    let member_bytes = entry.member_bytes()?;
    let member_pubkeys: Vec<PubKey> = member_bytes.iter().copied().map(PubKey::from).collect();
    let safe_members =
        SafeVec::<PubKey, MAX_ATTESTOR_SET_MEMBERS>::try_from(member_pubkeys.clone())
            .map_err(|e| anyhow!("attestor_members exceeds MAX_ATTESTOR_SET_MEMBERS: {e:?}"))?;
    let threshold_u8: u8 = entry
        .attestor_threshold
        .try_into()
        .map_err(|_| anyhow!("attestor_threshold {} exceeds u8", entry.attestor_threshold))?;
    let attestor_set_id = AttestorSet::derive_id(&member_pubkeys, threshold_u8);

    // 2. Hash the JSON Schema doc (canonical-form check inside).
    let spec_path = schema_root.join(&entry.spec_file);
    let payload_shape_hash = shape_hash::compute(&spec_path)?;

    // 3. Resolve schema id from owner + name + version.
    let schema_id = Schema::<S>::derive_id(owner_addr, &entry.name, entry.version);

    // 4. RegisterAttestorSet (idempotent).
    let attestor_set_status = if attestor_set_exists(rpc, &attestor_set_id).await? {
        TxOutcome::AlreadyRegistered
    } else {
        let call = RuntimeCall::Attestation(AttestationCall::RegisterAttestorSet {
            members: safe_members,
            threshold: threshold_u8,
        });
        submit_with_nonce(submitter, rpc, signer_key, chain_id, chain_hash, *nonce, max_fee, call)
            .await
            .context("submitting RegisterAttestorSet")?;
        *nonce += 1;
        TxOutcome::Submitted
    };

    // 5. RegisterSchema (idempotent).
    let schema_status = if schema_exists(rpc, &schema_id).await? {
        TxOutcome::AlreadyRegistered
    } else {
        let name = SafeString::try_from(entry.name.clone())
            .map_err(|e| anyhow!("schema name fails SafeString cap: {e:?}"))?;
        let fee_routing_addr = entry.fee_routing_addr.as_deref().map(parse_addr).transpose()?;
        let call = RuntimeCall::Attestation(AttestationCall::RegisterSchema {
            name,
            version: entry.version,
            attestor_set: attestor_set_id,
            fee_routing_bps: entry.fee_routing_bps,
            fee_routing_addr,
            payload_shape_hash,
        });
        submit_with_nonce(submitter, rpc, signer_key, chain_id, chain_hash, *nonce, max_fee, call)
            .await
            .context("submitting RegisterSchema")?;
        *nonce += 1;
        TxOutcome::Submitted
    };

    Ok(SchemaOutcome {
        name: entry.name.clone(),
        version: entry.version,
        attestor_set_id,
        schema_id,
        payload_shape_hash,
        attestor_set_status,
        schema_status,
    })
}

/// Wrap → sign → submit → poll a single `RuntimeCall`.
#[allow(clippy::too_many_arguments)]
async fn submit_with_nonce(
    submitter: &Submitter,
    rpc: &str,
    signer_key: &SovPrivateKey,
    chain_id: u64,
    chain_hash: &[u8; 32],
    nonce: u64,
    max_fee: u128,
    call: RuntimeCall<S>,
) -> Result<()> {
    let unsigned = UnsignedTransaction::<ChainRuntime, S>::new(
        call,
        chain_id,
        PriorityFeeBips::ZERO,
        Amount::from(max_fee),
        UniquenessData::Nonce(nonce),
        None,
    );
    let signed = unsigned.sign(signer_key, chain_hash);
    let signed_bytes = borsh::to_vec(&signed).context("encoding signed tx")?;
    let tx_hash =
        submitter.submit_raw_tx(signed_bytes, false).await.context("submitting tx to sequencer")?;
    let tx_hash_str = tx_hash.to_string();
    wait_for_inclusion_via_http(submitter, rpc, &tx_hash_str).await
}

/// Same HTTP-poll pattern as `ligate-cli`'s `wait_for_inclusion_via_http`.
/// 30-second timeout, 500ms poll interval. Polls
/// `GET {rpc}/ledger/txs/{hash}`.
async fn wait_for_inclusion_via_http(
    submitter: &Submitter,
    rpc_with_v1: &str,
    tx_hash: &str,
) -> Result<()> {
    const POLL_INTERVAL: Duration = Duration::from_millis(500);
    const MAX_WAIT: Duration = Duration::from_secs(30);

    let url = format!("{rpc_with_v1}/ledger/txs/{tx_hash}");
    let started = Instant::now();
    loop {
        if started.elapsed() > MAX_WAIT {
            bail!(
                "timed out after {MAX_WAIT:?} waiting for tx {tx_hash} to be included; \
                 the tx may still land — verify via {url}",
            );
        }
        if submitter.inner().http_get(&url).await.is_ok() {
            return Ok(());
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

async fn attestor_set_exists(rpc: &str, id: &AttestorSetId) -> Result<bool> {
    rest_get_exists(&format!("{rpc}/modules/attestation/attestor-sets/{id}")).await
}

async fn schema_exists(rpc: &str, id: &SchemaId) -> Result<bool> {
    rest_get_exists(&format!("{rpc}/modules/attestation/schemas/{id}")).await
}

/// Fire a `GET` and treat 2xx as "exists", 404 as "doesn't exist", any
/// other status as a hard error. We use a fresh `reqwest::Client` per
/// call rather than the SDK's because the SDK's `http_get` returns
/// `Err` on 404, which is ambiguous with network failures.
async fn rest_get_exists(url: &str) -> Result<bool> {
    let resp = reqwest::get(url).await.with_context(|| format!("HTTP GET {url}"))?;
    let status = resp.status();
    if status == reqwest::StatusCode::NOT_FOUND {
        return Ok(false);
    }
    if status.is_success() {
        return Ok(true);
    }
    let body = resp.text().await.unwrap_or_default();
    bail!("unexpected status {status} from {url}: {body}");
}

/// Idempotent `/v1` suffix. Mirrors the cli/faucet/js-SDK normalizers.
fn normalize_rpc(rpc: &str) -> String {
    let trimmed = rpc.trim_end_matches('/');
    if trimmed.ends_with("/v1") {
        trimmed.to_string()
    } else {
        format!("{trimmed}/v1")
    }
}

/// Derive the chain address from a 32-byte ed25519 pubkey.
///
/// Mirrors the chain's authentication path: `Address::from(pubkey[..28])`.
/// Uses `pubkey[..28]` (NOT `SHA-256(pubkey)[..28]`); see chain #245.
/// `credential_id()` takes no generic arg in the workspace pin; the
/// `Hasher` association lives on the trait impl.
fn pubkey_to_address(pubkey: &<<S as Spec>::CryptoSpec as CryptoSpec>::PublicKey) -> SovAddress {
    let id = pubkey.credential_id();
    SovAddress::from(id)
}

fn parse_addr(s: &str) -> Result<SovAddress> {
    use std::str::FromStr;
    SovAddress::from_str(s).map_err(|e| anyhow!("parsing fee_routing_addr {s}: {e:?}"))
}

/// Shape of `GET /v1/rollup/info`. Operators don't need to provide
/// `--chain-id` and `--chain-hash` if we can fetch them from the node
/// itself; this struct deserialises that fetch.
#[derive(Debug, Deserialize)]
pub struct RollupInfoResponse {
    pub chain_id: String,
    pub chain_hash: String,
    #[serde(default)]
    pub version: String,
}

/// Fetch chain identity from `/v1/rollup/info` so the operator can
/// run the ceremony with just `--rpc` + `--signer-key`.
pub async fn fetch_rollup_info(rpc: &str) -> Result<RollupInfoResponse> {
    let url = format!("{}/rollup/info", normalize_rpc(rpc));
    let resp = reqwest::get(&url)
        .await
        .with_context(|| format!("GET {url}"))?
        .error_for_status()
        .with_context(|| format!("non-2xx from {url}"))?;
    Ok(resp.json::<RollupInfoResponse>().await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_rpc_appends_v1_idempotently() {
        assert_eq!(normalize_rpc("http://localhost:12346"), "http://localhost:12346/v1");
        assert_eq!(normalize_rpc("http://localhost:12346/"), "http://localhost:12346/v1");
        assert_eq!(normalize_rpc("http://localhost:12346/v1"), "http://localhost:12346/v1");
        assert_eq!(normalize_rpc("http://localhost:12346/v1/"), "http://localhost:12346/v1");
    }
}
