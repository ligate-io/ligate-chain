//! End-to-end tests for the genesis tool.
//!
//! Each test invokes the binary as a child process (the unit tests
//! inside `src/main.rs` cover the substitution logic at function
//! granularity). This file covers the operator-facing surface:
//!
//! - `verify` succeeds on the canonical localnet bundle.
//! - `verify` fails with a useful error on a deliberately corrupted
//!   bundle.
//! - `generate` round-trips: empty substitutions on the localnet
//!   template produce a JSON-equivalent output (modulo serde-default
//!   pretty-print formatting) that itself passes `verify`.
//! - `generate` substitution actually replaces addresses.

use std::path::PathBuf;
use std::process::Command;

use tempfile::TempDir;

/// Path to the workspace root, regardless of where `cargo test` is
/// invoked from.
fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("crates/genesis-tool/ has a workspace-root grandparent")
        .to_path_buf()
}

fn devnet_genesis_dir() -> PathBuf {
    workspace_root().join("devnet").join("genesis")
}

/// Locate the freshly-built genesis-tool binary.
///
/// Cargo sets `CARGO_BIN_EXE_<bin-name>` when an integration test is
/// compiled, pointing at the built binary's location. Available on
/// every supported cargo version.
fn binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_ligate-genesis-tool"))
}

#[test]
fn verify_succeeds_on_localnet_bundle() {
    let output = Command::new(binary())
        .arg("verify")
        .arg("--dir")
        .arg(devnet_genesis_dir())
        .output()
        .expect("invoke binary");
    assert!(
        output.status.success(),
        "verify on devnet/genesis/ should succeed; stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("verify: OK"), "missing 'verify: OK' in stderr:\n{stderr}");
    assert!(stderr.contains("treasury:"), "missing summary lines:\n{stderr}");
}

#[test]
fn verify_fails_on_corrupted_bundle() {
    // Copy the localnet bundle to a temp dir, then mangle bank.json
    // so the gas-token config is unparseable. The validator should
    // surface the I/O / parse error rather than a panic.
    let tmp = TempDir::new().expect("tempdir");
    for entry in std::fs::read_dir(devnet_genesis_dir()).expect("read template") {
        let entry = entry.expect("dirent");
        let dst = tmp.path().join(entry.file_name());
        std::fs::copy(entry.path(), dst).expect("copy file");
    }
    std::fs::write(tmp.path().join("bank.json"), "{ this is not json }").expect("write corrupted");

    let output = Command::new(binary())
        .arg("verify")
        .arg("--dir")
        .arg(tmp.path())
        .output()
        .expect("invoke binary");
    assert!(
        !output.status.success(),
        "verify on a corrupted bundle should fail; stdout:\n{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("verify failed"), "missing 'verify failed' in stderr:\n{stderr}");
}

#[test]
fn generate_with_empty_substitutions_round_trips() {
    // Empty substitutions: tool reads the template, walks every JSON,
    // touches nothing, writes back. The output must verify cleanly.
    let tmp = TempDir::new().expect("tempdir");
    let output_dir = tmp.path().join("out");
    let subs_path = tmp.path().join("subs.toml");
    std::fs::write(&subs_path, "").expect("write empty substitutions");

    let result = Command::new(binary())
        .arg("generate")
        .arg("--template")
        .arg(devnet_genesis_dir())
        .arg("--substitutions")
        .arg(&subs_path)
        .arg("--output")
        .arg(&output_dir)
        .output()
        .expect("invoke binary");
    assert!(
        result.status.success(),
        "generate should succeed; stderr:\n{}",
        String::from_utf8_lossy(&result.stderr)
    );

    // The output bundle should pass verify on its own (the binary
    // already runs verify internally before exiting, but a separate
    // invocation pins the contract).
    let verify = Command::new(binary())
        .arg("verify")
        .arg("--dir")
        .arg(&output_dir)
        .output()
        .expect("invoke verify");
    assert!(
        verify.status.success(),
        "verify on generated output should succeed; stderr:\n{}",
        String::from_utf8_lossy(&verify.stderr)
    );
}

#[test]
fn generate_substitutes_addresses_and_balances() {
    // Substitute the bootstrap address with a different valid lig1
    // address and override its balance. The output should:
    //   - have the new address everywhere the old one appeared
    //   - have the new balance in bank.json
    //   - pass verify (no cross-module invariant is broken by the
    //     specific substitution)
    let tmp = TempDir::new().expect("tempdir");
    let output_dir = tmp.path().join("out");
    let subs_path = tmp.path().join("subs.toml");

    // Override bootstrap's balance to a value slightly above the
    // localnet default (100M $LGT in nanos). Any value above the
    // sum of bonds bootstrap holds across attester / prover /
    // sequencer would also work; we pick a specifically larger
    // number so the verify step (which the binary runs internally
    // post-generate) doesn't complain about insufficient funds for
    // the modules' bonds.
    let bootstrap = "lig1h72nh5c7jfjkcygku4thsh2t53dyh33kkpktpy84w06qwr4agvt";
    let new_balance = "200000000000000000"; // 200M $LGT in nanos
    let subs_toml = format!(
        r#"
[balances]
"{bootstrap}" = "{new_balance}"
"#,
    );
    std::fs::write(&subs_path, subs_toml).expect("write substitutions");

    let result = Command::new(binary())
        .arg("generate")
        .arg("--template")
        .arg(devnet_genesis_dir())
        .arg("--substitutions")
        .arg(&subs_path)
        .arg("--output")
        .arg(&output_dir)
        .output()
        .expect("invoke binary");
    assert!(
        result.status.success(),
        "generate should succeed; stderr:\n{}",
        String::from_utf8_lossy(&result.stderr)
    );

    // Inspect bank.json: bootstrap balance should now be "1".
    let bank_path = output_dir.join("bank.json");
    let bank_text = std::fs::read_to_string(&bank_path).expect("read bank.json");
    let bank_value: serde_json::Value = serde_json::from_str(&bank_text).expect("parse bank.json");
    let rows = bank_value["gas_token_config"]["address_and_balances"]
        .as_array()
        .expect("address_and_balances");
    let bootstrap_row =
        rows.iter().find(|row| row[0].as_str() == Some(bootstrap)).expect("bootstrap row exists");
    assert_eq!(bootstrap_row[1].as_str(), Some(new_balance), "balance was overridden");
}

#[test]
fn keys_generate_writes_files_and_prints_stub() {
    // Exercises the `keys generate` subcommand end-to-end via the
    // binary entrypoint — covers the `Keys` / `KeysAction` dispatch
    // arms in `main()` plus the full `run_keys_generate` body, which
    // the unit tests in `keys.rs` don't reach (those test the
    // `generate_role` library function directly).
    let tmp = TempDir::new().expect("tempdir");
    let output_dir = tmp.path().join("keys");

    let result = Command::new(binary())
        .arg("keys")
        .arg("generate")
        .arg("--roles")
        .arg("operator,demo1,demo2")
        .arg("--output")
        .arg(&output_dir)
        .output()
        .expect("invoke binary");
    assert!(
        result.status.success(),
        "keys generate should succeed; stderr:\n{}",
        String::from_utf8_lossy(&result.stderr)
    );

    // Stderr: human-friendly per-role summary.
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(stderr.contains("keys generate: OK"), "missing 'keys generate: OK':\n{stderr}");
    for role in ["operator", "demo1", "demo2"] {
        assert!(stderr.contains(role), "stderr missing role {role}:\n{stderr}");
    }
    assert!(stderr.contains("chmod 600"), "stderr missing chmod 600 reminder:\n{stderr}");

    // Stdout: keys.toml stub. Operators pipe this into devnet-1/keys.toml.
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(stdout.contains("[addresses]"), "stdout missing [addresses] header:\n{stdout}");
    assert!(
        stdout.contains("<placeholder-address>"),
        "stdout missing placeholder marker:\n{stdout}"
    );
    // Each role gets a comment + an entry in the stub.
    for role in ["operator", "demo1", "demo2"] {
        assert!(
            stdout.contains(&format!("# role: {role}")),
            "stub missing role comment {role}:\n{stdout}"
        );
    }

    // Disk: per role, two files (`<role>.key` chmod 600, `<role>.address`).
    use std::os::unix::fs::PermissionsExt;
    for role in ["operator", "demo1", "demo2"] {
        let key_path = output_dir.join(format!("{role}.key"));
        let addr_path = output_dir.join(format!("{role}.address"));
        assert!(key_path.exists(), "{} missing", key_path.display());
        assert!(addr_path.exists(), "{} missing", addr_path.display());

        let mode = std::fs::metadata(&key_path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "{} should be chmod 600, got {:o}", key_path.display(), mode);

        let addr = std::fs::read_to_string(&addr_path).unwrap();
        assert!(
            addr.starts_with("lig1"),
            "address file {} should start with lig1: {}",
            addr_path.display(),
            addr.trim()
        );
    }
}

#[test]
fn keys_generate_addresses_in_stub_match_address_files() {
    // The keys.toml stub on stdout should reference the same lig1
    // addresses that get written to the per-role .address files —
    // operators paste from stdout, the corresponding key files are
    // on disk, and the two views had better agree.
    let tmp = TempDir::new().expect("tempdir");
    let output_dir = tmp.path().join("keys");

    let result = Command::new(binary())
        .arg("keys")
        .arg("generate")
        .arg("--roles")
        .arg("operator")
        .arg("--output")
        .arg(&output_dir)
        .output()
        .expect("invoke binary");
    assert!(result.status.success(), "stderr: {}", String::from_utf8_lossy(&result.stderr));

    let on_disk =
        std::fs::read_to_string(output_dir.join("operator.address")).unwrap().trim().to_string();
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(
        stdout.contains(&on_disk),
        "stdout stub should reference the same address as operator.address ({on_disk}):\n{stdout}"
    );
}
