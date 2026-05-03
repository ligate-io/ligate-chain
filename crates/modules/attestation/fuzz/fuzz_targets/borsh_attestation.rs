//! Fuzz the Borsh decoder for `Attestation<S>`.
//!
//! State value behind `StateMap<AttestationId, Attestation<S>>`.
//! Each `Attestation` holds a `SignedAttestationPayload` plus a
//! quorum of attestor signatures; the decoder must reject malformed
//! signature buffers and out-of-bounds threshold counts without
//! panicking.

#![no_main]

use attestation::Attestation;
use libfuzzer_sys::fuzz_target;

type S = sov_test_utils::TestSpec;

fuzz_target!(|data: &[u8]| {
    let _ = borsh::from_slice::<Attestation<S>>(data);
});
