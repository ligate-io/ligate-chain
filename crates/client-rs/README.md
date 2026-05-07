# `ligate-client`

The Rust client SDK for Ligate Chain.

Two faces, gated behind a Cargo feature:

| Default (`ligate-client = "..."`) | With `features = ["submit"]` |
|---|---|
| Typed CallMessage builders for the attestation module | All of default, plus an HTTP submission pipeline |
| `register_attestor_set`, `register_schema`, `build_signed_submit` | A `Submitter` wrapping the SDK's `NodeClient` |
| Ed25519 signing helpers | `submit_raw_tx` + `submit_raw_txs` for batch submit |
| `attestation_digest` for schema-id and digest derivation | Re-exports `sov_node_client::NodeClient` for state queries |
| Pure typed-message use; embeddable in zkVM guest contexts | Brings tokio + reqwest; for host-side consumers only |

## Install

The chain's Sovereign SDK pin is git-based, so this crate (which transitively
depends on it) cannot be published to crates.io until upstream lands on
crates.io. Install via git:

```toml
[dependencies]
ligate-client = { git = "https://github.com/ligate-io/ligate-chain.git", tag = "v0.1.0-devnet" }

# Or with the submission pipeline:
ligate-client = { git = "https://github.com/ligate-io/ligate-chain.git", tag = "v0.1.0-devnet", features = ["submit"] }
```

Tracking issue for the crates.io publish path: [#235](https://github.com/ligate-io/ligate-chain/issues/235).

## Default usage: typed message construction

```rust,no_run
use ed25519_dalek::SigningKey;
use ligate_client::{
    build_signed_submit, register_attestor_set, register_schema, AttestorSet, PayloadHash,
    PubKey, Schema,
};
use sov_modules_api::{Address, Spec};

type S = sov_test_utils::TestSpec;

let signers: Vec<SigningKey> = (1u8..=3).map(|i| SigningKey::from_bytes(&[i; 32])).collect();
let members: Vec<PubKey> =
    signers.iter().map(|sk| PubKey::from(sk.verifying_key().to_bytes())).collect();

// Build the three CallMessages this SDK speaks today:
let _ = register_attestor_set::<S>(members.clone(), 2)?;
let attestor_set_id = AttestorSet::derive_id(&members, 2);
let _ = register_schema::<S>(
    "themisra.proof-of-prompt".into(),
    1, attestor_set_id, 0, None, [0u8; 32],
)?;
let submitter: <S as Spec>::Address = Address::from([1u8; 28]);
let schema_id = Schema::<S>::derive_id(&submitter, "themisra.proof-of-prompt", 1);
let payload_hash = PayloadHash::from([0x42u8; 32]);
let keys: Vec<&SigningKey> = signers.iter().take(2).collect();
let _ = build_signed_submit::<S>(&keys, schema_id, payload_hash, submitter, 0)?;
# Ok::<(), Box<dyn std::error::Error>>(())
```

## With `submit` feature: HTTP submission pipeline

The `submit` module wraps the SDK's `NodeClient` for the operator-facing
build-sign-submit-wait flow. See [`src/submit.rs`](src/submit.rs) for the
full module-level walkthrough; the punchline:

1. Build a runtime call (e.g. `RuntimeCall::Bank(bank::CallMessage::Transfer { ... })`)
2. Wrap in `UnsignedTransaction::new` with chain id, nonce, fees
3. Sign with `unsigned.sign(private_key, &chain_hash)` (binds to chain)
4. Borsh-encode + wrap in `AuthenticatorInput::Standard`
5. Submit via `Submitter::submit_raw_tx`

```rust,ignore
use ligate_client::submit::Submitter;

let submitter = Submitter::new("https://rpc.ligate.io").await?;

// Build + sign + encode (steps 1-4 above; uses SDK types).
// See faucet repo for a worked example.
let signed_tx_bytes: Vec<u8> = build_signed_tx_for_my_chain(/* ... */)?;

let tx_hash = submitter.submit_raw_tx(signed_tx_bytes, /* wait_for_inclusion */ true).await?;
```

For a full worked example (build a `bank.transfer`, sign with a hot key,
submit, wait for inclusion), see the faucet repo's `signer.rs`:
<https://github.com/ligate-io/faucet/blob/main/src/signer.rs>.

## Status

- ✅ Typed CallMessage builders (`register_attestor_set`, `register_schema`, `build_signed_submit`, `attestation_digest`, `sign_attestation`)
- ✅ Re-exports of all attestation protocol types
- ✅ HTTP submission pipeline (`submit` feature)
- ❌ Wallet / key management abstractions (out of scope; consumers manage their own keys)
- ❌ EVM-shaped transaction authentication ([#72](https://github.com/ligate-io/ligate-chain/issues/72), v4)
- ❌ TypeScript counterpart at `@ligate/client` ([#236](https://github.com/ligate-io/ligate-chain/issues/236), separate repo)

## Tracking

- [#21](https://github.com/ligate-io/ligate-chain/issues/21) — original SDK design
- [#235](https://github.com/ligate-io/ligate-chain/issues/235) — crates.io publish (gated on Sovereign SDK)
- [#240](https://github.com/ligate-io/ligate-chain/issues/240) — submission pipeline (closed by this crate's `submit` feature)

## License

Apache-2.0 OR MIT. See [`../../LICENSE-APACHE`](../../LICENSE-APACHE) and [`../../LICENSE-MIT`](../../LICENSE-MIT).
