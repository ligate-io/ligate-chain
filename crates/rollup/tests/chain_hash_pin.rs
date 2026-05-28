//! Pin the runtime's `CHAIN_HASH` to a known value.
//!
//! `CHAIN_HASH` is `hash(serialized wallet schema + chain data)`, computed
//! at build time by the macros in `sov-modules-stf-blueprint`. It changes
//! whenever the runtime composition changes: a new module added, a module
//! field reshaped, a config-value renamed, an SDK rev that touches any
//! schema-affecting type. Wallets, signed txs, and any partner that pinned
//! the value are tied to this hash for replay protection — silent drift
//! would invalidate every cached `chain_hash` we've handed out.
//!
//! This test exists to fail loudly when the value changes, so:
//!
//! 1. Whoever changed the schema notices in CI rather than at boot time.
//! 2. The `STATE.md` / partner integrations get an explicit signal to
//!    update their pinned value.
//! 3. Devnet operators have an early warning before they re-genesis.
//!
//! When the value legitimately changes (intentional schema bump,
//! deliberate runtime rev), update `EXPECTED_CHAIN_HASH` below and note
//! the reason in the commit message + `STATE.md`.

use ligate_rollup::MockRollupSpec;
use ligate_stf::Runtime;
use sov_modules_api::execution_mode::Native;
use sov_modules_api::Runtime as RuntimeTrait;

// `Spec` used by `MockLigateRollup` in `Native` execution mode. Resolves
// `<Runtime as RuntimeTrait>::CHAIN_HASH` without booting the full
// blueprint. CHAIN_HASH is computed at build time from the Runtime
// composition + chain data, so any execution mode produces the same
// value.
type Spec = MockRollupSpec<Native>;

/// Currently expected runtime `CHAIN_HASH`, hex-encoded (lowercase, no
/// `0x` prefix, 64 chars). Bumped when the `bounty` module was added
/// to the runtime composition in chain#519 (the v0 skeleton PR);
/// adding a module to the composition struct shifts the borsh schema
/// fingerprint that flows into `CHAIN_HASH`. Prior value (pre-bounty)
/// was `eec077f4736df42cddb547236468dad32f1fd6822aaad1e822ce596307552df2`.
///
/// Updates require: bump this value, update `STATE.md`, note the reason
/// in the commit message.
const EXPECTED_CHAIN_HASH: &str =
    "0db23038539976d0eaeb57fe4e6843d1e32dd66d428af400e9caace1fb180efc";

#[test]
fn chain_hash_matches_pinned_value() {
    let actual = <Runtime<Spec> as RuntimeTrait<Spec>>::CHAIN_HASH;
    let actual_hex = hex::encode(actual);
    assert_eq!(
        actual_hex, EXPECTED_CHAIN_HASH,
        "CHAIN_HASH drifted. Expected {EXPECTED_CHAIN_HASH}, got {actual_hex}. \
         If this is intentional (schema change, SDK rev bump), update \
         EXPECTED_CHAIN_HASH in this test and note the reason in STATE.md \
         and the commit message. Wallets / partners pinned to the old value \
         must be re-handed the new one before they sign new txs."
    );
}
