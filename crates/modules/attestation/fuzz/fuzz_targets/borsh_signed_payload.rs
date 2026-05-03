//! Fuzz the Borsh decoder for `SignedAttestationPayload<S>`.
//!
//! The payload an attestor signs over before producing an
//! `AttestorSignature`. Every quorum signature path round-trips
//! through this struct. Decoder panics here would propagate into
//! signature-validation flows.

#![no_main]

use attestation::SignedAttestationPayload;
use libfuzzer_sys::fuzz_target;

type S = sov_test_utils::TestSpec;

fuzz_target!(|data: &[u8]| {
    let _ = borsh::from_slice::<SignedAttestationPayload<S>>(data);
});
