//! Property-based codec roundtrip tests.
//!
//! Complements [`borsh_snapshot.rs`] (wire-format guard) with
//! random-input decode(encode(t)) == t coverage on the protocol's
//! public types. The two answer different questions:
//!
//! - Snapshots prove the wire format hasn't shifted (a specific
//!   input produces a specific byte pattern).
//! - Properties prove encode and decode are inverses (any valid
//!   input round-trips cleanly).
//!
//! Targets:
//!
//! - The 4 hash newtypes (`SchemaId`, `AttestorSetId`, `PayloadHash`,
//!   `PubKey`) — Borsh roundtrip on 32-byte content
//! - `AttestationId` — Borsh roundtrip AND Display/FromStr roundtrip
//! - `AttestorSet` — Borsh roundtrip with random member counts up to
//!   the cap
//! - `AttestorSignature` — Borsh roundtrip with random signature
//!   byte length up to the cap
//! - Decoder robustness — random bytes either decode cleanly or
//!   produce a typed error, never panic
//!
//! Tracking issue: ligate-io/ligate-chain#122.
//!
//! ## What this doesn't cover
//!
//! Types parameterised on `Spec` (`Schema<S>`, `Attestation<S>`,
//! `SignedAttestationPayload<S>`, `CallMessage<S>`) involve chain
//! `Address` strategy work that's deeper than this test file's
//! scope. The snapshot tests in `borsh_snapshot.rs` already
//! pin specific instances of those; a follow-up can add proptest
//! coverage when an `arbitrary` impl on `Address` is available
//! upstream. Filed as a follow-up against #122.

use attestation::{
    AttestationId, AttestorSet, AttestorSetId, AttestorSignature, PayloadHash, PubKey, SchemaId,
    MAX_ATTESTOR_SET_MEMBERS, MAX_ATTESTOR_SIGNATURE_BYTES,
};
use borsh::{BorshDeserialize, BorshSerialize};
use proptest::collection::vec;
use proptest::prelude::*;
use sov_modules_api::SafeVec;

// ----- Strategies -----------------------------------------------------------

fn schema_id_strategy() -> impl Strategy<Value = SchemaId> {
    any::<[u8; 32]>().prop_map(SchemaId::from)
}

fn attestor_set_id_strategy() -> impl Strategy<Value = AttestorSetId> {
    any::<[u8; 32]>().prop_map(AttestorSetId::from)
}

fn payload_hash_strategy() -> impl Strategy<Value = PayloadHash> {
    any::<[u8; 32]>().prop_map(PayloadHash::from)
}

fn pubkey_strategy() -> impl Strategy<Value = PubKey> {
    any::<[u8; 32]>().prop_map(PubKey::from)
}

fn attestation_id_strategy() -> impl Strategy<Value = AttestationId> {
    (schema_id_strategy(), payload_hash_strategy())
        .prop_map(|(s, p)| AttestationId::from_pair(&s, &p))
}

/// `AttestorSet` with members count in `[1, MAX_ATTESTOR_SET_MEMBERS]`
/// and threshold a valid `[1, members.len()]` value. Outside those
/// bounds is rejected at validation time; `AttestorSet` itself is
/// Borsh-encodable without the check, so we test the encoder against
/// the full valid input space.
fn attestor_set_strategy() -> impl Strategy<Value = AttestorSet> {
    (1usize..=MAX_ATTESTOR_SET_MEMBERS).prop_flat_map(|n| {
        let members = vec(pubkey_strategy(), n);
        let threshold = 1u8..=(n as u8);
        (members, threshold).prop_map(|(members, threshold)| AttestorSet { members, threshold })
    })
}

/// `AttestorSignature` with random pubkey + signature bytes capped
/// at `MAX_ATTESTOR_SIGNATURE_BYTES`. Real signatures are ed25519
/// (64 bytes) but the type permits a range.
fn attestor_signature_strategy() -> impl Strategy<Value = AttestorSignature> {
    (pubkey_strategy(), vec(any::<u8>(), 1..=MAX_ATTESTOR_SIGNATURE_BYTES)).prop_map(
        |(pubkey, sig_bytes)| {
            let sig = SafeVec::<u8, MAX_ATTESTOR_SIGNATURE_BYTES>::try_from(sig_bytes)
                .expect("strategy enforces length cap");
            AttestorSignature { pubkey, sig }
        },
    )
}

// ----- Roundtrip properties --------------------------------------------------

/// Helper: assert `decode(encode(t)) == t` for any `T: BorshSerialize + BorshDeserialize`.
fn assert_borsh_roundtrip<T>(value: T)
where
    T: BorshSerialize + BorshDeserialize + PartialEq + std::fmt::Debug,
{
    let encoded = borsh::to_vec(&value).expect("encode");
    let decoded = T::try_from_slice(&encoded).expect("decode");
    assert_eq!(decoded, value, "borsh roundtrip mismatch");
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 256,
        ..ProptestConfig::default()
    })]

    #[test]
    fn schema_id_borsh_roundtrips(id in schema_id_strategy()) {
        assert_borsh_roundtrip(id);
    }

    #[test]
    fn attestor_set_id_borsh_roundtrips(id in attestor_set_id_strategy()) {
        assert_borsh_roundtrip(id);
    }

    #[test]
    fn payload_hash_borsh_roundtrips(h in payload_hash_strategy()) {
        assert_borsh_roundtrip(h);
    }

    #[test]
    fn pubkey_borsh_roundtrips(pk in pubkey_strategy()) {
        assert_borsh_roundtrip(pk);
    }

    #[test]
    fn attestation_id_borsh_roundtrips(id in attestation_id_strategy()) {
        assert_borsh_roundtrip(id);
    }

    #[test]
    fn attestation_id_string_roundtrips(id in attestation_id_strategy()) {
        let s = id.to_string();
        let parsed: AttestationId = s.parse().expect("display form should parse");
        prop_assert_eq!(parsed, id);
    }

    #[test]
    fn attestor_set_borsh_roundtrips(set in attestor_set_strategy()) {
        assert_borsh_roundtrip(set);
    }

    #[test]
    fn attestor_signature_borsh_roundtrips(sig in attestor_signature_strategy()) {
        assert_borsh_roundtrip(sig);
    }
}

// ----- Decoder robustness ----------------------------------------------------
//
// Random bytes fed to `try_from_slice` should never panic; the
// decoder must return either `Ok(t)` (with a re-encodable `t`) or a
// typed `borsh::io::Error`. Tests one bug class proptest is great at:
// trusting input length / discriminant bytes without bounds-checking
// could cause an out-of-bounds read on the decoder side.

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 1024,
        ..ProptestConfig::default()
    })]

    #[test]
    fn schema_id_decoder_never_panics(bytes in vec(any::<u8>(), 0..64)) {
        let _ = SchemaId::try_from_slice(&bytes);
    }

    #[test]
    fn attestor_set_id_decoder_never_panics(bytes in vec(any::<u8>(), 0..64)) {
        let _ = AttestorSetId::try_from_slice(&bytes);
    }

    #[test]
    fn payload_hash_decoder_never_panics(bytes in vec(any::<u8>(), 0..64)) {
        let _ = PayloadHash::try_from_slice(&bytes);
    }

    #[test]
    fn pubkey_decoder_never_panics(bytes in vec(any::<u8>(), 0..64)) {
        let _ = PubKey::try_from_slice(&bytes);
    }

    #[test]
    fn attestation_id_decoder_never_panics(bytes in vec(any::<u8>(), 0..128)) {
        let _ = AttestationId::try_from_slice(&bytes);
    }

    #[test]
    fn attestor_set_decoder_never_panics(bytes in vec(any::<u8>(), 0..4096)) {
        // 4 KB upper bound covers a maxed-out set:
        // (32 bytes/key * MAX_ATTESTOR_SET_MEMBERS) + (4-byte length) + (1-byte threshold)
        let _ = AttestorSet::try_from_slice(&bytes);
    }

    #[test]
    fn attestor_signature_decoder_never_panics(bytes in vec(any::<u8>(), 0..512)) {
        // 32-byte pubkey + (4-byte length) + sig up to MAX_ATTESTOR_SIGNATURE_BYTES.
        let _ = AttestorSignature::try_from_slice(&bytes);
    }

    #[test]
    fn attestation_id_str_decoder_never_panics(s in ".*") {
        // Arbitrary UTF-8 strings; FromStr must error cleanly on
        // anything that's not a valid `lat1...` bech32m string, not
        // panic.
        let _: Result<AttestationId, _> = s.parse();
    }
}
