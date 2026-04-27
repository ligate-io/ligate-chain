//! Risc0 prover artifacts for the Ligate Chain rollup binary.
//!
//! Re-exports the constants the build script generated:
//!
//! - `ROLLUP_ELF: &[u8]` — the guest binary, fed into
//!   `sov_risc0_adapter::host::Risc0Host::new`.
//! - `ROLLUP_ID: [u32; 8]` — the image hash. Pin this in
//!   `devnet/genesis/chain_state.json` as the runtime's
//!   `inner_code_commitment`.
//! - `ROLLUP_PATH: &str` — the on-disk path of the compiled ELF
//!   (debugging only).
//!
//! Under `SKIP_GUEST_BUILD=1` the constants are zero / empty
//! placeholders. See `build.rs` for the gate.

// `methods.rs` is build-script-generated — its constants come
// without doc comments, so we can't `#![deny(missing_docs)]` here.
// The constants are documented at the module level above.
#![allow(missing_docs)]

include!(concat!(env!("OUT_DIR"), "/methods.rs"));
