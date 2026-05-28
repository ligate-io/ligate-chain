//! Integration tests for the bounty module's v0 skeleton.
//!
//! Coverage at this stage is intentionally narrow: the module ships
//! with no-op handlers, so the only behavioural surface to assert is
//! "the chain accepts the CallMessage shape without panicking and
//! emits no state changes." Real state-transition assertions land in
//! the follow-up PRs against
//! [chain#519](https://github.com/ligate-io/ligate-chain/issues/519).
//!
//! What we do test:
//!
//! - [`BountyId::derive`] is deterministic given the same inputs.
//! - [`BountyId::derive`] is distinct for distinct nonces.
//! - The `DisputeKey` `FromStr`/`Display` round-trip is lossless.

use std::str::FromStr;

use attestation::{AttestationId, SchemaId};
use bounty::{BountyId, DisputeKey};

#[test]
fn bounty_id_derive_is_deterministic() {
    let poster = b"lid1examplebountyposteraddressbytes";
    let board_schema_id = SchemaId::from([0x42u8; 32]);
    let nonce = 7u64;

    let id_a = BountyId::derive(poster, &board_schema_id, nonce);
    let id_b = BountyId::derive(poster, &board_schema_id, nonce);

    assert_eq!(id_a, id_b, "BountyId::derive must be deterministic on identical inputs");
}

#[test]
fn bounty_id_derive_distinguishes_nonces() {
    let poster = b"lid1examplebountyposteraddressbytes";
    let board_schema_id = SchemaId::from([0x42u8; 32]);

    let id_n0 = BountyId::derive(poster, &board_schema_id, 0);
    let id_n1 = BountyId::derive(poster, &board_schema_id, 1);

    assert_ne!(
        id_n0, id_n1,
        "BountyId::derive must produce distinct ids for distinct nonces with same poster + board"
    );
}

#[test]
fn bounty_id_derive_distinguishes_boards() {
    let poster = b"lid1examplebountyposteraddressbytes";
    let board_a = SchemaId::from([0x42u8; 32]);
    let board_b = SchemaId::from([0xABu8; 32]);

    let id_a = BountyId::derive(poster, &board_a, 0);
    let id_b = BountyId::derive(poster, &board_b, 0);

    assert_ne!(
        id_a, id_b,
        "BountyId::derive must produce distinct ids for distinct boards with same poster + nonce"
    );
}

#[test]
fn dispute_key_string_roundtrip() {
    let bounty_id = BountyId::from([0x11u8; 32]);
    let attestation_id = AttestationId::from([0x22u8; 32]);
    let key = DisputeKey { bounty_id, attestation_id };

    let s = key.to_string();
    assert!(s.starts_with("lbt1"), "DisputeKey Display must start with the bounty Bech32 prefix");
    assert!(s.contains(':'), "DisputeKey Display must use ':' between the two ids");
    assert!(s.contains("lat1"), "DisputeKey Display must include the attestation Bech32 segment");

    let parsed = DisputeKey::from_str(&s).expect("FromStr round-trip must succeed");
    assert_eq!(parsed.bounty_id, key.bounty_id);
    assert_eq!(parsed.attestation_id, key.attestation_id);
}

#[test]
fn dispute_key_from_str_rejects_malformed() {
    // Missing the separator.
    assert!(DisputeKey::from_str("lbt1abclat1xyz").is_err());
    // Empty input.
    assert!(DisputeKey::from_str("").is_err());
    // Bad bech32m hrp on the bounty side.
    assert!(DisputeKey::from_str("xxx1abc:lat1xyz").is_err());
}
