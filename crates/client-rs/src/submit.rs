//! Transaction submission pipeline (feature-gated under `submit`).
//!
//! Wraps the Sovereign SDK's [`NodeClient`] for the operator-facing
//! "build, sign, submit, wait" pipeline that downstream consumers
//! (`ligate-io/faucet`, `ligate-io/ligate-cli`, integration tests)
//! need to actually move state on a running chain.
//!
//! Tracking issue: <https://github.com/ligate-io/ligate-chain/issues/240>.
//!
//! # Why this is feature-gated
//!
//! The default `ligate-client` build is purely typed message
//! construction. It builds attestation `CallMessage`s, signs
//! attestations, derives schema / attestor-set ids. It does not
//! touch tokio, reqwest, or any HTTP plumbing. That keeps the
//! crate cheap to depend on for in-zkVM guest usage and for
//! lightweight test harnesses.
//!
//! The `submit` feature opts into the network-side pieces:
//! `NodeClient`, async tokio, and the SDK's transaction envelope.
//! Faucet / cli / integration tests enable it; pure-message
//! consumers do not.
//!
//! # Build-sign-submit walkthrough
//!
//! 1. **Build a runtime call.** Each chain composes its modules
//!    into a `Runtime` whose `DispatchCall::Decodable` is the
//!    runtime-level call enum (e.g. `RuntimeCall::Bank(...)`,
//!    `RuntimeCall::AttestationModule(...)`). Use the chain's
//!    re-exported type from `ligate_stf::runtime::RuntimeCall`.
//!
//! 2. **Wrap in `UnsignedTransaction`.** The SDK ships
//!    [`sov_modules_api::transaction::UnsignedTransaction`] which
//!    binds the runtime call to a chain id, a nonce / uniqueness
//!    marker, and gas / fee parameters.
//!
//! 3. **Sign.** `unsigned.sign(private_key, &chain_hash)` returns
//!    a [`sov_modules_api::transaction::Transaction`]. The signature
//!    is domain-separated to the chain's `CHAIN_HASH` constant
//!    (build-time hash of the runtime composition + spec). A
//!    transaction for `ligate-devnet-1` does not verify on
//!    `ligate-devnet-2` even if the signing key is identical.
//!
//! 4. **Encode.** Borsh-serialize the signed transaction into raw
//!    bytes, then wrap in `AuthenticatorInput::Standard(...)` per
//!    the chain's `RollupAuthenticator`, and Borsh-serialize again.
//!    The double-wrap is the SDK's authentication-scheme contract.
//!
//! 5. **Submit.** [`Submitter::submit_raw_tx`] hands the bytes to
//!    the sequencer's `accept_tx` endpoint and (optionally) polls
//!    `wait_for_tx_processing` until the chain has executed it.
//!
//! ## Important: do NOT pre-wrap on the client
//!
//! The chain's `POST /v1/sequencer/txs` handler ([`axum_accept_tx`])
//! wraps the request body in `AuthenticatorInput::Standard(RawTx {
//! data })` server-side. **Pass the borsh-encoded `Transaction` bytes
//! directly to `submit_raw_tx`; do NOT call `encode_with_standard_auth`
//! on the client.** Pre-wrapping double-wraps and the chain rejects
//! with `signature error: Cannot decompress Edwards point`. See
//! [`ligate-chain#245`].
//!
//! See the integration test in `tests/submit.rs` for a worked example
//! (when localnet harness is available) and the faucet repo's
//! `signer.rs` for production usage.
//!
//! [`axum_accept_tx`]: https://github.com/Sovereign-Labs/sovereign-sdk/blob/f89964c/crates/full-node/sov-sequencer/src/rest_api.rs
//! [`ligate-chain#245`]: https://github.com/ligate-io/ligate-chain/issues/245
//!
//! # Why not a single high-level helper?
//!
//! The natural "convenience" function would take a runtime call +
//! signing key and do steps 2-5 internally. That requires generic
//! bounds on the runtime type that quickly become opaque
//! (`TransactionCallable`, `CryptoSpec`, `Spec::CryptoSpec::PrivateKey`,
//! etc.). Splitting steps 2-4 to the consumer (which already has
//! the runtime type in scope) keeps this module ergonomic and
//! avoids a wall of `where` clauses. The faucet's signer module is
//! ~15 lines using this surface; that's the right ergonomic floor
//! for pre-mainnet operator tools.

use anyhow::{Context, Result};
use sov_node_client::NodeClient;

/// Re-export of the SDK's full-node REST client. Use the methods on
/// it directly for state queries (`get_balance`, `get_nonce_for_public_key`,
/// etc.). [`Submitter`] is just a thin wrapper for the submit-tx +
/// wait-for-processing path that downstream consumers want to
/// always pair.
pub use sov_node_client::NodeClient as InnerClient;

/// Thin wrapper around [`NodeClient`] for the submit-tx pipeline.
///
/// Holds an HTTP client to a running `ligate-node`'s REST API
/// (default port `12346`, public domain `rpc.ligate.io`). One
/// instance is cheap to clone (`reqwest::Client` is `Arc`-shared
/// internally); construct one per process and clone into tasks.
#[derive(Debug, Clone)]
pub struct Submitter {
    inner: NodeClient,
}

impl Submitter {
    /// Construct a [`Submitter`] without verifying the target URL.
    ///
    /// Use this when you've already confirmed the URL externally or
    /// when the target node is not yet reachable at construct time
    /// (e.g., a faucet starting before its sequencer comes up).
    /// Subsequent calls fail at request time.
    pub fn new_unchecked(rpc_url: &str) -> Self {
        Self { inner: NodeClient::new_unchecked(rpc_url) }
    }

    /// Construct a [`Submitter`] and verify the target URL is alive
    /// + serving the expected runtime schema. Fails fast if the
    /// node is unreachable.
    pub async fn new(rpc_url: &str) -> Result<Self> {
        let inner = NodeClient::new(rpc_url)
            .await
            .with_context(|| format!("connecting to ligate-node at {rpc_url}"))?;
        Ok(Self { inner })
    }

    /// Borrow the inner [`NodeClient`] for state queries
    /// (`get_balance`, `get_nonce_for_public_key`, etc.) that this
    /// wrapper doesn't re-export.
    pub fn inner(&self) -> &NodeClient {
        &self.inner
    }

    /// Submit a borsh-encoded signed transaction to the chain's
    /// sequencer. Returns the transaction hash.
    ///
    /// `bytes` MUST be `borsh::to_vec(&signed_transaction)` where
    /// `signed_transaction` is a `Transaction<R, S>` produced by
    /// [`UnsignedTransaction::sign`]. The chain's
    /// `POST /v1/sequencer/txs` handler wraps the bytes in
    /// `AuthenticatorInput::Standard(RawTx { data })` server-side;
    /// do NOT pre-wrap on the client (see module docs for why).
    ///
    /// Garbage in produces a server-side `FatalError::DeserializationFailed`
    /// rather than a panic.
    ///
    /// `wait_for_inclusion = true` polls `wait_for_tx_processing`
    /// after submission. Returns once the chain has executed the
    /// transaction (success or failure both count). Polling timeout
    /// is governed by the SDK's default backoff (~30 seconds).
    pub async fn submit_raw_tx(
        &self,
        bytes: Vec<u8>,
        wait_for_inclusion: bool,
    ) -> Result<sov_api_spec::types::TxHash> {
        let mut hashes = self
            .inner
            .send_transactions_to_sequencer(vec![bytes], wait_for_inclusion)
            .await
            .context("submitting transaction to sequencer")?;
        hashes.pop().context("sequencer accepted the request but returned no tx hashes")
    }

    /// Convenience: submit a batch of pre-built transactions in
    /// a single sequencer call. Useful for high-throughput consumers
    /// (load tests, batch operations); for one-off submissions use
    /// [`Submitter::submit_raw_tx`].
    pub async fn submit_raw_txs(
        &self,
        batch: Vec<Vec<u8>>,
        wait_for_inclusion: bool,
    ) -> Result<Vec<sov_api_spec::types::TxHash>> {
        self.inner
            .send_transactions_to_sequencer(batch, wait_for_inclusion)
            .await
            .context("submitting transaction batch to sequencer")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_unchecked_does_not_block() {
        // Construction without a reachable node should not error.
        // The Faucet repo relies on this for startup ordering: faucet
        // can boot, then later submit when the sequencer comes up.
        let s = Submitter::new_unchecked("http://localhost:12346");
        let _ = s.inner();
    }
}

/// Type aliases that downstream consumers commonly want from this
/// module without re-importing from `sov_node_client::types`.
pub mod types {
    pub use sov_api_spec::types::TxHash;
}
