//! Fuzz the Borsh decoder for `CallMessage<S>`.
//!
//! `CallMessage` is the chain's primary user-facing tx payload; every
//! transaction the sequencer accepts gets routed through this decoder.
//! A panic here is a free DoS vector. The Borsh snapshot tests in
//! `tests/borsh_snapshot.rs` lock the encoding; this fuzz proves the
//! decoder is panic-safe under arbitrary bytes.

#![no_main]

use attestation::CallMessage;
use libfuzzer_sys::fuzz_target;

type S = sov_test_utils::TestSpec;

fuzz_target!(|data: &[u8]| {
    let _ = borsh::from_slice::<CallMessage<S>>(data);
});
