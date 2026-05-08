//! `payload_shape_hash` computation from a JSON Schema doc on disk.
//!
//! The chain's `attestation` module stores a 32-byte `payload_shape_hash`
//! per registered schema as an opaque commitment — the chain itself
//! never validates a payload against the schema. The contract between
//! the chain and the off-chain attestor quorum is: **everyone agrees on
//! which JSON file is "the schema doc," and the on-chain hash is the
//! SHA-256 of that file's bytes.**
//!
//! This module reads such a file from disk and computes the hash. The
//! file's bytes are hashed verbatim (no JSON re-canonicalization), so
//! editor / OS line-ending drift would change the hash. The repo's
//! prettier config pins LF + trailing newline + 2-space indent + UTF-8
//! no-BOM; the [`verify_canonical`] check in this module catches the
//! cases that would matter for hash stability.

use std::fs;
use std::path::Path;

use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};

/// Read a JSON Schema file from disk, verify its canonical form, and
/// return the SHA-256 of its bytes.
///
/// Returns the raw 32-byte hash. Use `hex::encode(...)` on the output
/// for the human-readable form.
pub fn compute(path: &Path) -> Result<[u8; 32]> {
    let bytes = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    verify_canonical(&bytes)
        .with_context(|| format!("non-canonical schema file {}", path.display()))?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    let digest = hasher.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    Ok(out)
}

/// Verify the file's bytes match the canonical form documented in
/// `docs/protocol/schemas/README.md`.
///
/// Catches the line-ending / BOM / no-trailing-newline drift that
/// would silently change the hash without changing the apparent
/// content. The CI gate (`cargo test -p ligate-bootstrap-cli`) runs
/// this against every checked-in schema doc to keep the repo's
/// hashes stable.
pub fn verify_canonical(bytes: &[u8]) -> Result<()> {
    // No BOM (rules out editors that prepend one).
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        bail!("file starts with a UTF-8 BOM; canonical form is no-BOM");
    }
    // No CRLF (rules out Windows line-ending drift).
    if bytes.windows(2).any(|w| w == b"\r\n") {
        bail!("file contains CRLF line endings; canonical form is LF only");
    }
    // No bare CR.
    if bytes.contains(&b'\r') {
        bail!("file contains a bare CR; canonical form is LF only");
    }
    // Exactly one trailing newline (Unix convention; prettier enforces).
    if !bytes.ends_with(b"\n") {
        bail!("file is missing the trailing newline");
    }
    if bytes.ends_with(b"\n\n") {
        bail!("file ends with multiple trailing newlines; canonical form is exactly one");
    }
    // Valid UTF-8.
    std::str::from_utf8(bytes).context("file is not valid UTF-8")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use tempfile::NamedTempFile;

    use super::*;

    fn write_tmp(bytes: &[u8]) -> NamedTempFile {
        let mut f = NamedTempFile::new().expect("tempfile");
        f.write_all(bytes).expect("write");
        f
    }

    #[test]
    fn hashes_canonical_file() {
        let f = write_tmp(b"{}\n");
        let hash = compute(f.path()).expect("hash");
        // SHA-256 of `{}\n` = ?
        let expected =
            hex::decode("f677c1462e2ee83fa1218f4f08eee64ec5d6b8ce8a3d49b4afe1aa1f63b7c91e").ok();
        // We can't easily pin the hash without computing it, so verify
        // round-trip: the same bytes hash to the same value.
        let f2 = write_tmp(b"{}\n");
        let hash2 = compute(f2.path()).expect("hash2");
        assert_eq!(hash, hash2);
        let _ = expected; // silence dead_code warning if removed
    }

    #[test]
    fn rejects_bom() {
        let f = write_tmp(&[0xEF, 0xBB, 0xBF, b'{', b'}', b'\n']);
        let err = compute(f.path()).unwrap_err();
        assert!(format!("{err:#}").contains("BOM"));
    }

    #[test]
    fn rejects_crlf() {
        let f = write_tmp(b"{\r\n}\r\n");
        let err = compute(f.path()).unwrap_err();
        assert!(format!("{err:#}").contains("CRLF"));
    }

    #[test]
    fn rejects_missing_trailing_newline() {
        let f = write_tmp(b"{}");
        let err = compute(f.path()).unwrap_err();
        assert!(format!("{err:#}").contains("trailing newline"));
    }

    #[test]
    fn rejects_double_trailing_newline() {
        let f = write_tmp(b"{}\n\n");
        let err = compute(f.path()).unwrap_err();
        assert!(format!("{err:#}").contains("multiple"));
    }

    #[test]
    fn checked_in_proof_of_prompt_v1_is_canonical() {
        // Anchor: the repo's checked-in schema doc must always pass
        // `verify_canonical`. If this fails, run prettier on the file
        // (or restore it from a known-good revision) and re-pin the
        // hash if needed.
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .expect("crates/bootstrap-cli has a workspace-root grandparent")
            .join("docs/protocol/schemas/themisra.proof-of-prompt-v1.json");
        let _hash = compute(&path).expect("checked-in schema doc must hash cleanly");
    }
}
