//! Fuzz the Borsh decoder for `AttestorSet`.
//!
//! State value behind `StateMap<AttestorSetId, AttestorSet>`. Same
//! threat model as `borsh_schema`: any panic here is a node-halt
//! vector if state can be poisoned upstream.

#![no_main]

use attestation::AttestorSet;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = borsh::from_slice::<AttestorSet>(data);
});
