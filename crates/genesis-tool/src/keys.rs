//! Key generation for operator-controlled Ligate addresses.
//!
//! Generates Ed25519 keypairs and derives the corresponding 28-byte
//! Ligate addresses (`lig1...`) using the SDK's standard derivation:
//! `Address = SHA-256(pubkey)[..28]`.
//!
//! Per role this writes two files into the output dir:
//!
//! - `<role>.key`     — hex-encoded 32-byte private key, terminated
//!   with `\n`. File mode `0600` (operator-only readable).
//! - `<role>.address` — `lig1...` bech32m string, terminated with `\n`.
//!
//! Stand-in for `ligate-cli keys generate` ([#112]). When `ligate-cli`
//! ships in its own repo, this module's logic moves there and
//! `ligate-genesis-tool` either re-exports or wraps it. Until then,
//! the operator workflow in `devnet-1/README.md` calls into this
//! subcommand directly.
//!
//! [#112]: https://github.com/ligate-io/ligate-chain/issues/112

use std::fs;
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use ed25519_dalek::SigningKey;
use rand::rngs::OsRng;
use rand::RngCore;
use sha2::{Digest, Sha256};
use sov_modules_api::Address;

/// Result of generating one role-tagged keypair.
///
/// `key_path` and `address_path` are part of the API surface even
/// though `main.rs` only reads `role` and `address` for stdout — the
/// path fields are read by the unit tests below, and are useful for
/// any future caller that wants to verify file placement (e.g., when
/// `ligate-cli` lifts this module wholesale per [#112]).
///
/// [#112]: https://github.com/ligate-io/ligate-chain/issues/112
#[allow(dead_code)]
pub struct GeneratedKey {
    /// The role label passed in (e.g., `"operator"`, `"demo1"`).
    pub role: String,
    /// The derived `lig1...` bech32m address, ready to paste into a
    /// `keys.toml` substitution map.
    pub address: String,
    /// Path to the private-key file (hex, chmod 600).
    pub key_path: PathBuf,
    /// Path to the address file (plaintext lig1... + newline).
    pub address_path: PathBuf,
}

/// Generate one Ed25519 keypair, derive its `lig1...` address, and
/// persist both to disk.
///
/// The output dir is created if it doesn't exist. The private-key
/// file is written with mode `0600` so only the running operator
/// can read it.
pub fn generate_role(role: &str, output_dir: &Path) -> Result<GeneratedKey> {
    fs::create_dir_all(output_dir)
        .with_context(|| format!("creating output dir {}", output_dir.display()))?;

    // Sample 32 bytes of CSPRNG entropy and construct the signing
    // key directly. Avoids needing the `rand_core` feature on
    // ed25519-dalek (workspace pin uses `default-features = false`).
    let mut secret_bytes = [0u8; 32];
    OsRng.fill_bytes(&mut secret_bytes);
    let signing_key = SigningKey::from_bytes(&secret_bytes);
    let pubkey_bytes = signing_key.verifying_key().to_bytes();

    // Address derivation: SHA-256(pubkey)[..28]. Matches the pattern
    // used in `crates/stf/tests/devnet_addresses.rs` for deterministic
    // devnet fixtures, but here the pubkey is the input rather than a
    // string label.
    let digest = Sha256::digest(pubkey_bytes);
    let mut addr_bytes = [0u8; 28];
    addr_bytes.copy_from_slice(&digest[..28]);
    let address = Address::from(addr_bytes);
    let address_str = address.to_string();

    let key_path = output_dir.join(format!("{role}.key"));
    let address_path = output_dir.join(format!("{role}.address"));

    // Write the private key with restrictive perms BEFORE writing
    // contents. Anyone observing the open(2) → write(2) sequence
    // sees the file at 0600 from creation, never at the umask
    // default.
    {
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&key_path)
            .with_context(|| format!("creating {}", key_path.display()))?;
        let key_hex = hex::encode(signing_key.to_bytes());
        f.write_all(key_hex.as_bytes())?;
        f.write_all(b"\n")?;
        // Belt-and-braces: `mode()` on `OpenOptions` only sets the
        // mode on file creation. Re-applying via `set_permissions`
        // covers the case where the file already existed (and
        // `truncate(true)` rewrote it).
        fs::set_permissions(&key_path, fs::Permissions::from_mode(0o600))?;
    }

    fs::write(&address_path, format!("{address_str}\n"))
        .with_context(|| format!("writing {}", address_path.display()))?;

    Ok(GeneratedKey { role: role.to_string(), address: address_str, key_path, address_path })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn generates_lig1_address() {
        let dir = tempdir().unwrap();
        let g = generate_role("operator", dir.path()).unwrap();

        // Address looks right.
        assert!(g.address.starts_with("lig1"), "got {}", g.address);

        // Key file is 64 hex chars + newline = 65 bytes, mode 0600.
        let key_bytes = fs::read(&g.key_path).unwrap();
        assert_eq!(key_bytes.len(), 65, "key file should be 64 hex + newline");
        let mode = fs::metadata(&g.key_path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "key file should be chmod 600, got {mode:o}");

        // Address file is the address string + newline.
        let addr_bytes = fs::read_to_string(&g.address_path).unwrap();
        assert_eq!(addr_bytes.trim_end_matches('\n'), g.address);
    }

    #[test]
    fn keys_are_independent() {
        let dir = tempdir().unwrap();
        let a = generate_role("a", dir.path()).unwrap();
        let b = generate_role("b", dir.path()).unwrap();
        assert_ne!(a.address, b.address, "two keys must produce two addresses");
        let key_a = fs::read(&a.key_path).unwrap();
        let key_b = fs::read(&b.key_path).unwrap();
        assert_ne!(key_a, key_b, "two keys must produce two private keys");
    }

    #[test]
    fn role_with_special_chars_is_namespaced() {
        // Role names get used verbatim as filename stems. Make sure
        // standard naming works; we don't try to sanitize because the
        // CLI rejects anything not in the user's input.
        let dir = tempdir().unwrap();
        let g = generate_role("sequencer-hot", dir.path()).unwrap();
        assert!(g.key_path.ends_with("sequencer-hot.key"));
        assert!(g.address_path.ends_with("sequencer-hot.address"));
    }
}
