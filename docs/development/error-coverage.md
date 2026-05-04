# Error-variant coverage audit

Catalogue of every public `Error` variant in the protocol crates plus the test that asserts it fires. Audit-prep hygiene from [#124](https://github.com/ligate-io/ligate-chain/issues/124).

The acceptance criterion: every `pub` Error variant has at least one test asserting it fires on the expected bad input. Tests that previously matched on `TxEffect::Reverted(_)` without pinning the variant now use the `assert_reverted_with(receipt, phrase)` helper to grep the rendered error message for a unique substring from the variant's `#[error("...")]` attribute, so a refactor that changes which variant fires breaks the test loudly.

## `attestation::AttestationError` (18 variants)

All in `crates/modules/attestation/src/lib.rs`. Tests in `crates/modules/attestation/tests/integration.rs` unless noted.

| Variant | Phrase pinned | Discriminant | Test |
|---|---|---|---|
| `ThresholdExceedsMembers` | `exceeds member count` | `threshold_exceeds_members` | `register_attestor_set_rejects_threshold_above_members` |
| `ZeroThreshold` | `must be at least 1` | `zero_threshold` | `register_attestor_set_rejects_zero_threshold` |
| `EmptyAttestorSet` | `must have at least one member` | `empty_attestor_set` | `register_attestor_set_rejects_empty_members` |
| `DuplicateAttestorSet` | `attestor set already registered` | `duplicate_attestor_set` | `register_attestor_set_rejects_duplicate` |
| `DuplicateSchema` | `schema already registered` | `duplicate_schema` | `register_schema_rejects_duplicate` |
| `UnknownAttestorSet` | `unregistered attestor set` | `unknown_attestor_set` | `register_schema_requires_existing_attestor_set` |
| `FeeRoutingExceedsCap` | `exceeds protocol cap` | `fee_routing_exceeds_cap` | `register_schema_rejects_over_cap_fee_routing` + `register_schema_rejects_above_custom_cap` |
| `OrphanFeeRouting` | `fee routing misconfigured` | `orphan_fee_routing` | `register_schema_rejects_orphan_routing_bps_without_addr` + `register_schema_rejects_orphan_routing_addr_without_bps` |
| `UnknownSchema` | `schema not found` | `unknown_schema` | `submit_attestation_requires_existing_schema` + `rejection_counter_bumps_on_typed_attestation_error` |
| `DuplicateAttestation` | `attestation already exists` | `duplicate_attestation` | `full_register_register_submit_flow` (write-once branch) |
| `DuplicateSignaturePubkey` | `duplicate attestor pubkey` | `duplicate_signature_pubkey` | `submit_with_duplicate_pubkey_rejects` |
| `UnknownAttestorPubkey` | `not in the schema's attestor set` | `unknown_attestor_pubkey` | `submit_with_unknown_pubkey_rejects` |
| `InvalidAttestorPubkey` | `not a valid ed25519 point` | `invalid_attestor_pubkey` | `submit_with_off_curve_pubkey_rejects` |
| `BadSignatureLength` | `expected 64 for ed25519` | `bad_signature_length` | `submit_with_bad_sig_length_rejects` |
| `InvalidSignature` | `did not verify` | `invalid_signature` | `submit_with_invalid_signature_rejects` + `submit_digest_is_bound_to_submitter` |
| `BelowThreshold` | `quorum not met` | `below_threshold` | `submit_below_threshold_rejects` + `submit_attestation_rejects_empty_signatures` |
| `MissingAttestorSet` | n/a (unreachable in v0) | `missing_attestor_set` | see "Unreachable variants" below |
| `ChainNotConfigured` | n/a (unreachable in tests) | `chain_not_configured` | see "Unreachable variants" below |

**Discriminant column** is the stable snake_case label produced by `AttestationError::discriminant()`. It's the value of the `reason` label on the Prometheus `ligate_attestations_rejected_total{reason}` counter. The `discriminant_labels_are_stable_and_unique` unit test in `tests/unit.rs` pins the mapping; renaming a label is a coordinated dashboard-update event, not a Rust-only change.

## `ligate_stf::genesis_config::GenesisError` (4 variants)

All in `crates/stf/src/genesis_config.rs`. Tests in `crates/stf/src/tests.rs::genesis_loader`.

| Variant | Test |
|---|---|
| `Io` | `create_genesis_config_surfaces_io_error_with_path` |
| `ParseJson` | `create_genesis_config_surfaces_parse_error_with_path` |
| `MissingGasToken` | `validate_rejects_missing_gas_token` |
| `LgtTokenIdMismatch` | `validate_rejects_lgt_token_id_mismatch` |

## `ligate_client::ClientError` (4 variants)

All in `crates/client-rs/src/lib.rs`. Tests in the same file's `mod tests` block.

| Variant | Test |
|---|---|
| `AttestorSetMembersOverCap` | `register_attestor_set_rejects_over_cap` |
| `AttestationSignaturesOverCap` | `submit_attestation_rejects_over_cap_signatures` |
| `SchemaNameOverCap` | `register_schema_rejects_over_cap_name` |
| `SignatureBytesOverCap` | n/a (currently unreachable) |

## Unreachable variants

Three variants have no test by design. Each is documented here so a future audit doesn't re-flag them.

### `AttestationError::MissingAttestorSet`

Fires when a `Schema` references an `AttestorSetId` that no longer exists in `StateMap<AttestorSetId, AttestorSet>`. v0 has no delete operation on attestor sets; once registered, an attestor set cannot disappear from state. The variant exists as defensive depth for a future v1+ feature that adds attestor-set retirement (governance-driven or auto-expiring sets); when that lands, pair the new feature with a test that fires this variant.

### `AttestationError::ChainNotConfigured`

Fires when the attestation module is invoked before `seed_from_config` has run (i.e., `lgt_token_id` or `treasury` is `None` in state). Every test path goes through `TestRunner` which always seeds via genesis, so this code path isn't reachable from `TransactionTestCase`. Two ways to fire it would be:

1. A direct module-level unit test that constructs an `AttestationModule` without genesis seeding and calls a handler. Plumbing-heavy because it requires building a `Context` + `TxState` by hand.
2. An end-to-end test that boots a runtime without an `AttestationConfig` (which currently fails at `validate_config` time, not at handler time).

Neither test pulls its weight today. The variant guards against a chain composition that omits the attestation genesis loader, and the existing `validate_config` cross-module check catches that case before any tx executes. Documenting as defensive depth, no test.

### `ClientError::SignatureBytesOverCap`

Fires when a caller-supplied signature byte string exceeds `MAX_ATTESTOR_SIGNATURE_BYTES` (96). The current public builders (`register_attestor_set`, `register_schema`, `submit_attestation`, `sign_attestation`) all produce `SafeVec<u8, MAX_ATTESTOR_SIGNATURE_BYTES>` from sources that are bounded by construction (`SigningKey::sign` always returns 64 bytes for ed25519). No public path produces an oversized sig bytes input.

The variant is reserved for a future builder that takes raw caller-supplied sig bytes (e.g. a `submit_with_raw_signatures` for cross-signature-scheme testing or a `submit_unverified` for relay scenarios). When such a builder lands, pair it with a test that fires this variant. Until then, the variant is callable in theory but not from any code in this repo.

## Helper: `assert_reverted_with`

```rust
#[track_caller]
fn assert_reverted_with(receipt: &TxEffect<S>, phrase: &str) {
    match receipt {
        TxEffect::Reverted(payload) => {
            let msg = payload.reason.to_string();
            assert!(
                msg.contains(phrase),
                "expected phrase '{phrase}' in reverted reason, got: {msg}",
            );
        }
        other => panic!("expected Reverted, got {other:?}"),
    }
}
```

Lives in `crates/modules/attestation/tests/integration.rs`. Use it in every new negative-path test instead of `assert!(matches!(_, TxEffect::Reverted(_)))`. The phrase argument should be a substring of the variant's `#[error("...")]` attribute that's unique enough not to match any other variant.

When a new `pub Error` variant ships, do all three of:

1. Add the `#[error("...")]` attribute with a phrase nobody else uses.
2. Write the test that fires it.
3. Add a row to the table above.

This document is the single source of truth for which variants are tested and which are intentionally unreachable. Refresh on every PR that touches a public Error enum.

## Out of scope

- **Coverage measurement** (`cargo llvm-cov`). "Every variant has a test" is the proxy we use; line coverage is a separate decision.
- **Property-based testing of decoders.** That's `proptest` ([#122](https://github.com/ligate-io/ligate-chain/issues/122)) territory.
- **Fuzzing of decoders.** Already shipped via [#119](https://github.com/ligate-io/ligate-chain/issues/119); see `crates/modules/attestation/fuzz/`.
