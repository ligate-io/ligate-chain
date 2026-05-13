//! Genesis tool for the Ligate Chain.
//!
//! Three subcommands:
//!
//! - `verify`: re-runs the on-chain genesis validator (same code path
//!   `ligate-node` runs at startup) against a directory of genesis
//!   JSONs, without booting the chain. Fast feedback for operators
//!   iterating on a hand-crafted bundle. Catches:
//!   - Schema drift in any per-module config
//!   - The cross-module `attestation.lgt_token_id == bank.gas_token_id` invariant
//!   - The 7 typed `GenesisError` variants for `AttestationConfig`
//!     (#175): empty members, invalid threshold, duplicate set,
//!     unknown set reference, duplicate schema, orphan routing,
//!     over-cap fee routing.
//!
//! - `generate`: substitutes addresses in an existing genesis bundle
//!   to produce a new one with operator-controlled keys. The expected
//!   workflow:
//!
//!   1. Operator generates ed25519 keys offline (via `ligate-cli` or
//!      similar; #112).
//!   2. Operator writes a substitution TOML mapping placeholder
//!      addresses (from a committed template like `devnet-1/genesis/`)
//!      to their real keys.
//!   3. Tool walks every JSON in the template, replaces address
//!      strings per the substitution map, writes to the output dir.
//!   4. Tool runs `verify` on the output before returning.
//!
//! - `keys generate`: stand-in for `ligate-cli keys generate` (#112).
//!   Generates one or more Ed25519 keypairs, derives `lig1...`
//!   addresses (`SHA-256(pubkey)[..28]`), and persists each role's
//!   private key + address to disk under `<output>/<role>.{key,address}`
//!   with the private-key file mode `0600`. Prints a `keys.toml`
//!   stub to stdout that the operator pastes into
//!   `devnet-1/keys.toml`. Used in step 1 of the operator workflow
//!   (#231); when `ligate-cli` ships, this subcommand is a candidate
//!   for re-export from there.
//!
//! Tracking issues:
//! - <https://github.com/ligate-io/ligate-chain/issues/191> (this tool)
//! - <https://github.com/ligate-io/ligate-chain/issues/231> (operator
//!   workflow that consumes `keys generate` + `generate`)
//! - <https://github.com/ligate-io/ligate-chain/issues/112> (the
//!   future home: `ligate-cli`)

mod keys;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{anyhow, Context};
use clap::{Parser, Subcommand};
use ligate_rollup::{CelestiaRollupSpec, MockRollupSpec};
use ligate_stf::genesis_config::{create_genesis_config, GenesisPaths};
use serde::Deserialize;
use sov_modules_api::execution_mode::Native;

/// Concrete specs used to drive the genesis loader.
///
/// `MockRollupSpec<Native>` and `CelestiaRollupSpec<Native>` both carry
/// `MultiAddressEvm` (the production address shape) so the tool's
/// deserialization matches what the `ligate-node` binary does at
/// startup. They differ only in their DA spec, and that difference
/// matters for the genesis bundle: `sequencer_registry.json`'s
/// `seq_da_address` field is `<S::Da as DaSpec>::Address`, which
/// `MockDaSpec` parses as 32-byte hex and `CelestiaSpec` parses as
/// bech32 `celestia1...`. A bundle that's valid for one DA is
/// generally invalid for the other.
///
/// `verify` / `generate` take a `--da` flag selecting which spec to
/// validate against. Default is `celestia` (production target). Mock
/// flow is opt-in for localnet / cross-OS-determinism tests. See
/// issue #325 for history.
type MockSpec = MockRollupSpec<Native>;
type CelestiaSpec = CelestiaRollupSpec<Native>;

#[derive(clap::ValueEnum, Clone, Copy, Debug, Default)]
enum DaFlavor {
    /// Mock DA (32-byte hex `seq_da_address`). Use for localnet,
    /// integration tests, cross-OS genesis-determinism CI.
    Mock,
    /// Celestia DA (bech32 `celestia1...` `seq_da_address`). Use for
    /// `ligate-devnet-1` and any production-shape deployment.
    #[default]
    Celestia,
}

/// The 8 module JSON filenames the genesis loader expects, in
/// `Module::genesis` declaration order. Matches
/// `ligate_stf::genesis_config::GenesisPaths::from_dir`.
const GENESIS_FILES: &[&str] = &[
    "bank.json",
    "accounts.json",
    "sequencer_registry.json",
    "operator_incentives.json",
    "attester_incentives.json",
    "prover_incentives.json",
    "chain_state.json",
    "attestation.json",
];

#[derive(Parser, Debug)]
#[command(
    name = "ligate-genesis-tool",
    version,
    about = "Generate and verify Ligate Chain genesis bundles",
    long_about = None,
)]
struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Run the cross-module genesis validator against a directory of
    /// JSONs. Same code path `ligate-node` runs at startup, but
    /// without booting the chain.
    Verify {
        /// Directory containing the 8 module JSONs.
        #[arg(long, value_name = "DIR")]
        dir: PathBuf,
        /// Which DA spec to validate against. Default is `celestia`
        /// (production target). Pass `--da mock` for localnet /
        /// integration / cross-OS-determinism CI.
        #[arg(long, value_enum, default_value_t = DaFlavor::Celestia)]
        da: DaFlavor,
    },

    /// Substitute addresses in a template genesis bundle to produce a
    /// new bundle with operator-controlled keys. Runs `verify` on the
    /// output before returning.
    Generate {
        /// Template directory (e.g., `devnet-1/genesis/`).
        #[arg(long, value_name = "DIR")]
        template: PathBuf,
        /// Substitution TOML (see top-of-file docs for shape).
        #[arg(long, value_name = "FILE")]
        substitutions: PathBuf,
        /// Output directory. Created if it doesn't exist.
        #[arg(long, value_name = "DIR")]
        output: PathBuf,
        /// Which DA spec the substituted output targets. Drives the
        /// post-generate verify pass. Default is `celestia`.
        #[arg(long, value_enum, default_value_t = DaFlavor::Celestia)]
        da: DaFlavor,
    },

    /// Generate one or more Ed25519 keypairs and derive their
    /// `lig1...` addresses, ready for the genesis substitution flow.
    /// See `src/keys.rs` for the on-disk shape.
    Keys {
        #[command(subcommand)]
        action: KeysAction,
    },
}

#[derive(Subcommand, Debug)]
enum KeysAction {
    /// Generate one keypair per role, persist to `<output>/<role>.key`
    /// (chmod 600) and `<output>/<role>.address`, and print a
    /// `devnet-1/keys.toml` stub to stdout.
    Generate {
        /// Comma-separated role names. Each role gets its own
        /// keypair. Example: `--roles operator,demo1,demo2`.
        #[arg(long, value_name = "ROLES", value_delimiter = ',', required = true)]
        roles: Vec<String>,
        /// Output directory for `<role>.key` and `<role>.address`.
        /// Created if absent. The directory itself is NOT mode-
        /// protected (only the `.key` files are 0600); use a
        /// dedicated dir like `~/.ligate-keys/devnet-1/`.
        #[arg(long, value_name = "DIR")]
        output: PathBuf,
    },
}

/// Schema of the substitution TOML.
///
/// ```toml
/// # operator-keys.toml
/// [addresses]
/// # placeholder address from the template -> operator-controlled address
/// "lig1placeholder..." = "lig1real..."
///
/// # Optional: override token balances. Key is the post-substitution
/// # address (i.e. the value side of [addresses]); value is the new
/// # nano-LGT balance to write into bank.json's `address_and_balances`.
/// [balances]
/// "lig1real..." = "100000000000000000"
/// ```
#[derive(Debug, Deserialize, Default)]
struct Substitutions {
    /// Address substitution map. Both sides are bech32 strings.
    #[serde(default)]
    addresses: BTreeMap<String, String>,
    /// Optional balance override map. Key is the post-substitution
    /// address; value is the nano-LGT balance as a decimal string
    /// (matches the bank module's serde shape for `Amount`).
    #[serde(default)]
    balances: BTreeMap<String, String>,
}

fn main() -> ExitCode {
    let args = Args::parse();
    match args.command {
        Command::Verify { dir, da } => match run_verify(&dir, da) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("verify failed: {e:#}");
                ExitCode::FAILURE
            }
        },
        Command::Generate { template, substitutions, output, da } => {
            match run_generate(&template, &substitutions, &output, da) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("generate failed: {e:#}");
                    ExitCode::FAILURE
                }
            }
        }
        Command::Keys { action } => match action {
            KeysAction::Generate { roles, output } => match run_keys_generate(&roles, &output) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("keys generate failed: {e:#}");
                    ExitCode::FAILURE
                }
            },
        },
    }
}

/// Generate keypairs for each requested role and emit a `keys.toml`
/// stub on stdout the operator can paste into `devnet-1/keys.toml`.
///
/// The stub leaves the LHS placeholder addresses unfilled — the
/// operator chooses which placeholder maps to which generated role
/// based on the role intent table in `devnet-1/README.md`.
fn run_keys_generate(roles: &[String], output: &Path) -> anyhow::Result<()> {
    if roles.is_empty() {
        return Err(anyhow!("at least one role is required (e.g., --roles operator)"));
    }

    let mut generated = Vec::with_capacity(roles.len());
    for role in roles {
        let g = keys::generate_role(role, output)
            .with_context(|| format!("generating keys for role '{role}'"))?;
        generated.push(g);
    }

    eprintln!("keys generate: OK");
    eprintln!("  output dir: {}", output.display());
    for g in &generated {
        eprintln!("  {role:>16}: {addr}", role = g.role, addr = g.address);
    }
    eprintln!();
    eprintln!("Private keys are at <output>/<role>.key (chmod 600). Never commit them.");

    // Stub goes to stdout (separate from the human-friendly summary
    // on stderr above) so operators can pipe it: `... | tee -a
    // devnet-1/keys.toml`.
    println!();
    println!("# Paste below into devnet-1/keys.toml. Replace each");
    println!("# \"<placeholder-address>\" with the placeholder you want");
    println!("# this role to substitute (see devnet-1/keys.toml.example");
    println!("# for the role -> placeholder mapping).");
    println!("[addresses]");
    for g in &generated {
        println!("# role: {}", g.role);
        println!("\"<placeholder-address>\" = \"{}\"", g.address);
    }

    Ok(())
}

/// Re-run the genesis loader's typed validator.
///
/// `create_genesis_config` reads each JSON, deserializes against the
/// `Runtime`'s expected `GenesisConfig<S>`, and runs
/// `validate_config` (cross-module invariants). Either step surfaces
/// as a typed `GenesisError` that this command prints verbatim, so
/// the operator's iteration loop is "edit JSON, run verify, read
/// error, repeat".
fn run_verify(dir: &Path, da: DaFlavor) -> anyhow::Result<()> {
    if !dir.is_dir() {
        return Err(anyhow!("not a directory: {}", dir.display()));
    }
    let paths = GenesisPaths::from_dir(dir);

    // The `seq_da_address` field is `<S::Da as DaSpec>::Address`; its
    // serde shape depends on the chosen DA. Validate against whichever
    // spec matches the operator's deployment target.
    let (treasury, lgt_token_id, n_sets, n_schemas) = match da {
        DaFlavor::Celestia => {
            let cfg = create_genesis_config::<CelestiaSpec>(&paths).with_context(|| {
                format!("genesis bundle at {} failed Celestia-DA validation", dir.display())
            })?;
            (
                cfg.attestation.treasury.to_string(),
                cfg.attestation.lgt_token_id.to_string(),
                cfg.attestation.initial_attestor_sets.len(),
                cfg.attestation.initial_schemas.len(),
            )
        }
        DaFlavor::Mock => {
            let cfg = create_genesis_config::<MockSpec>(&paths).with_context(|| {
                format!("genesis bundle at {} failed Mock-DA validation", dir.display())
            })?;
            (
                cfg.attestation.treasury.to_string(),
                cfg.attestation.lgt_token_id.to_string(),
                cfg.attestation.initial_attestor_sets.len(),
                cfg.attestation.initial_schemas.len(),
            )
        }
    };

    eprintln!("verify: OK (DA={:?})", da);
    eprintln!("  treasury: {}", treasury);
    eprintln!("  lgt_token_id: {}", lgt_token_id);
    eprintln!("  initial_attestor_sets: {}", n_sets);
    eprintln!("  initial_schemas: {}", n_schemas);
    Ok(())
}

/// Apply substitutions to a template bundle and write a fresh one.
///
/// 1. Read the substitutions TOML.
/// 2. For each of the 8 known JSON files, read it as a `serde_json::Value`,
///    walk recursively, and replace any string that's a key in
///    `subs.addresses` with the mapped value.
/// 3. For balance overrides: walk `bank.json`'s
///    `gas_token_config.address_and_balances` array and replace the
///    second element (the balance) for any address whose key is in
///    `subs.balances`. The bank module serializes `Amount` as a
///    decimal string, so we substitute strings, not integers.
/// 4. Write the rewritten JSONs to the output directory, pretty-printed
///    with the same 2-space indent the existing templates use.
/// 5. Run `verify` on the output. Substitution errors that pass
///    syntactic validation but break a cross-module invariant
///    (e.g. introducing a duplicate address that violates a uniqueness
///    check) surface here.
fn run_generate(
    template: &Path,
    subs_path: &Path,
    output: &Path,
    da: DaFlavor,
) -> anyhow::Result<()> {
    let subs_text = std::fs::read_to_string(subs_path)
        .with_context(|| format!("read substitutions from {}", subs_path.display()))?;
    let subs: Substitutions = toml::from_str(&subs_text)
        .with_context(|| format!("parse substitutions TOML at {}", subs_path.display()))?;

    if !template.is_dir() {
        return Err(anyhow!("template is not a directory: {}", template.display()));
    }
    std::fs::create_dir_all(output)
        .with_context(|| format!("create output directory {}", output.display()))?;

    for filename in GENESIS_FILES {
        let src = template.join(filename);
        let dst = output.join(filename);
        let raw = std::fs::read_to_string(&src)
            .with_context(|| format!("read template file {}", src.display()))?;
        let mut value: serde_json::Value = serde_json::from_str(&raw)
            .with_context(|| format!("parse template JSON at {}", src.display()))?;

        substitute_addresses(&mut value, &subs.addresses);

        if *filename == "bank.json" {
            apply_balance_overrides(&mut value, &subs.balances);
        }

        let mut out_text = serde_json::to_string_pretty(&value)
            .with_context(|| format!("re-serialize {}", filename))?;
        // Match the trailing-newline convention of the committed templates.
        out_text.push('\n');
        std::fs::write(&dst, out_text).with_context(|| format!("write {}", dst.display()))?;
    }

    eprintln!("generate: wrote {} files to {}", GENESIS_FILES.len(), output.display());
    eprintln!("generate: validating output (DA={:?})...", da);
    run_verify(output, da)?;
    Ok(())
}

/// Recursively walk a JSON value, replacing any string that's a key
/// in the substitution map with the mapped value.
///
/// Conservative: only replaces full-string matches. A substring of
/// some longer string (e.g., a hex blob containing the address bytes
/// inline) is left alone, on the principle that an operator can't
/// reason about partial replacements as easily as full-string ones.
fn substitute_addresses(value: &mut serde_json::Value, addresses: &BTreeMap<String, String>) {
    match value {
        serde_json::Value::String(s) => {
            if let Some(replacement) = addresses.get(s) {
                *s = replacement.clone();
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                substitute_addresses(item, addresses);
            }
        }
        serde_json::Value::Object(map) => {
            for (_, v) in map.iter_mut() {
                substitute_addresses(v, addresses);
            }
        }
        // Numbers, bools, nulls: nothing to substitute.
        _ => {}
    }
}

/// Apply per-address balance overrides to `bank.json`.
///
/// Targets the path:
/// `gas_token_config.address_and_balances[i] = [address, balance_str]`.
///
/// A no-op if `gas_token_config` is missing or differently shaped;
/// the subsequent `verify` call catches that case with the SDK's
/// typed error rather than our own one.
fn apply_balance_overrides(value: &mut serde_json::Value, balances: &BTreeMap<String, String>) {
    if balances.is_empty() {
        return;
    }
    let Some(gas_token) = value.get_mut("gas_token_config") else {
        return;
    };
    let Some(rows) = gas_token.get_mut("address_and_balances").and_then(|v| v.as_array_mut())
    else {
        return;
    };
    for row in rows.iter_mut() {
        let Some(pair) = row.as_array_mut() else {
            continue;
        };
        if pair.len() != 2 {
            continue;
        }
        let Some(addr) = pair[0].as_str().map(str::to_owned) else {
            continue;
        };
        if let Some(new_balance) = balances.get(&addr) {
            pair[1] = serde_json::Value::String(new_balance.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn substitute_addresses_replaces_full_string_match() {
        let mut value = serde_json::json!({
            "owner": "lig1placeholder",
            "balances": [
                ["lig1placeholder", "1000"],
                ["lig1other", "500"]
            ],
            "nested": {"addr": "lig1placeholder"}
        });
        let mut subs = BTreeMap::new();
        subs.insert("lig1placeholder".to_string(), "lig1real".to_string());
        substitute_addresses(&mut value, &subs);
        let expected = serde_json::json!({
            "owner": "lig1real",
            "balances": [
                ["lig1real", "1000"],
                ["lig1other", "500"]
            ],
            "nested": {"addr": "lig1real"}
        });
        assert_eq!(value, expected);
    }

    #[test]
    fn substitute_addresses_leaves_substring_matches_alone() {
        let mut value = serde_json::json!({
            "blob": "lig1placeholderXXXXandmore",
            "exact": "lig1placeholder"
        });
        let mut subs = BTreeMap::new();
        subs.insert("lig1placeholder".to_string(), "lig1real".to_string());
        substitute_addresses(&mut value, &subs);
        // The blob field is unchanged: substring matches don't substitute.
        assert_eq!(value["blob"], "lig1placeholderXXXXandmore");
        assert_eq!(value["exact"], "lig1real");
    }

    #[test]
    fn balance_overrides_targets_only_gas_token_config() {
        let mut bank = serde_json::json!({
            "gas_token_config": {
                "token_name": "$LGT",
                "address_and_balances": [
                    ["lig1real", "1000"],
                    ["lig1other", "500"]
                ]
            },
            "tokens": []
        });
        let mut overrides = BTreeMap::new();
        overrides.insert("lig1real".to_string(), "999999999".to_string());
        apply_balance_overrides(&mut bank, &overrides);
        assert_eq!(bank["gas_token_config"]["address_and_balances"][0][1], "999999999");
        assert_eq!(bank["gas_token_config"]["address_and_balances"][1][1], "500");
    }

    #[test]
    fn balance_overrides_no_op_on_missing_gas_token_config() {
        let mut bank = serde_json::json!({"tokens": []});
        let mut overrides = BTreeMap::new();
        overrides.insert("lig1x".to_string(), "1".to_string());
        // Should not panic; the verify step will catch the structural
        // problem with a typed error.
        apply_balance_overrides(&mut bank, &overrides);
        assert_eq!(bank, serde_json::json!({"tokens": []}));
    }
}
