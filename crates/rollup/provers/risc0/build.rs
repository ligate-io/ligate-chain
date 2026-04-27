//! Compiles the risc0 guest crate(s) into ELF binaries.
//!
//! Honors `SKIP_GUEST_BUILD=1`: when set, the script emits empty
//! ELF bytes and a zero image ID instead of running the full
//! risc0 toolchain. This is the dev-loop default in CI — actual
//! proving runs in a separate workflow that explicitly unsets it.
//!
//! Mirrors the SDK demo's `provers/risc0/build.rs`. New guest
//! sub-crates (e.g. a future Mock-DA guest) get added to
//! [`get_guest_options`].

use std::collections::HashMap;

use sov_zkvm_utils::should_skip_guest_build;

fn main() {
    println!("cargo::rerun-if-env-changed=SKIP_GUEST_BUILD");

    if should_skip_guest_build("risc0") {
        println!("cargo:warning=Skipping risc0 guest build");
        // When skipping, don't track guest source changes — only
        // re-run if `build.rs` itself changes.
        println!("cargo::rerun-if-changed=build.rs");

        let out_dir = std::env::var_os("OUT_DIR").unwrap();
        let methods_path = std::path::Path::new(&out_dir).join("methods.rs");

        // Empty placeholders. Consumers (`src/lib.rs` `include!`s
        // this file) get well-typed `ROLLUP_ELF: &[u8]` and
        // `ROLLUP_ID: [u32; 8]` constants either way; they're
        // just zero / empty in skip mode.
        let empty = r#"
            pub const ROLLUP_PATH: &str = "";
            pub const ROLLUP_ELF: &[u8] = b"";
            pub const ROLLUP_ID: [u32; 8] = [0; 8];
        "#;

        std::fs::write(methods_path, empty).expect("Failed to write skip-mode methods.rs");
    } else {
        println!("cargo::rerun-if-env-changed=OUT_DIR");
        let guest_pkg_to_options = get_guest_options();
        risc0_build::embed_methods_with_options(guest_pkg_to_options);
    }
}

fn get_guest_options() -> HashMap<&'static str, risc0_build::GuestOptions> {
    let mut guest_pkg_to_options = HashMap::new();
    let features = sov_zkvm_utils::collect_features(&["bench", "bincode"], &["native"]);
    let guest_options =
        risc0_build::GuestOptionsBuilder::default().features(features).build().unwrap();
    guest_pkg_to_options.insert("ligate-prover-guest-celestia-risc0", guest_options);
    guest_pkg_to_options
}
