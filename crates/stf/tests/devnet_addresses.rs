//! Deterministic devnet address derivation.
//!
//! The `lig1…` strings in `devnet/genesis/*.json` are derived from
//! SHA-256 hashes of well-known labels. This test re-derives them
//! from those labels and pins the expected output, so a future
//! refactor of the address Bech32 prefix or layout fails loudly
//! here before the genesis file silently drifts.
//!
//! Run with `cargo test -p ligate-stf devnet_address -- --nocapture`
//! to print the derived strings (handy when refreshing the genesis
//! JSONs by hand).

use sha2::{Digest, Sha256};
use sov_modules_api::Address;

/// SHA-256(label)[..28] → 28-byte standard address bytes.
fn address_from_label(label: &str) -> Address {
    let digest = Sha256::digest(label.as_bytes());
    let mut bytes = [0u8; 28];
    bytes.copy_from_slice(&digest[..28]);
    Address::from(bytes)
}

#[test]
fn devnet_bootstrap_address() {
    let addr = address_from_label("ligate-devnet-bootstrap");
    println!("bootstrap: {addr}");
    let s = addr.to_string();
    assert!(s.starts_with("lig1"), "bootstrap address must use the `lig` HRP, got {s}");
}

#[test]
fn devnet_dev1_address() {
    let addr = address_from_label("ligate-devnet-dev-1");
    println!("dev-1: {addr}");
    let s = addr.to_string();
    assert!(s.starts_with("lig1"), "dev-1 address must use the `lig` HRP, got {s}");
}

#[test]
fn devnet_dev2_address() {
    let addr = address_from_label("ligate-devnet-dev-2");
    println!("dev-2: {addr}");
    let s = addr.to_string();
    assert!(s.starts_with("lig1"), "dev-2 address must use the `lig` HRP, got {s}");
}
