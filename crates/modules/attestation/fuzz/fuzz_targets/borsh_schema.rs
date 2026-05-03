//! Fuzz the Borsh decoder for `Schema<S>`.
//!
//! `Schema<S>` is read-side state: the value sitting behind
//! `StateMap<SchemaId, Schema<S>>`. A corrupted state byte stream
//! must surface as a typed error, not a panic, otherwise an
//! attacker who can poison the state tree (e.g. via a malicious
//! DA blob) could halt nodes.

#![no_main]

use attestation::Schema;
use libfuzzer_sys::fuzz_target;

type S = sov_test_utils::TestSpec;

fuzz_target!(|data: &[u8]| {
    let _ = borsh::from_slice::<Schema<S>>(data);
});
