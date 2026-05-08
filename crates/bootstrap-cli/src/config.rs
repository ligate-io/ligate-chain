//! TOML config types for the `register-canonical-schemas` subcommand.
//!
//! Operators describe the schemas they want to register in a TOML
//! file (typically committed to the repo alongside the chain config,
//! e.g. `devnet-1/canonical-schemas.toml`). Each `[[schemas]]` table
//! is one full registration: the JSON Schema doc to hash, the
//! attestor-set members + threshold, and the fee-routing config.
//!
//! Shape (one or more `[[schemas]]` blocks):
//!
//! ```toml
//! # Where the JSON Schema docs live, relative to this TOML file's
//! # directory. Optional; defaults to "../docs/protocol/schemas" so
//! # devnet-1/canonical-schemas.toml resolves to the chain's checked-in
//! # schema docs without further configuration.
//! schema_root = "../docs/protocol/schemas"
//!
//! [[schemas]]
//! name = "themisra.proof-of-prompt"
//! version = 1
//! spec_file = "themisra.proof-of-prompt-v1.json"
//! attestor_threshold = 2
//! attestor_members = [
//!   "<64-char hex-encoded ed25519 pubkey>",
//!   "<64-char hex-encoded ed25519 pubkey>",
//!   "<64-char hex-encoded ed25519 pubkey>",
//! ]
//! fee_routing_bps = 0
//! # fee_routing_addr = "lig1..."  # optional; absent means no routing
//! ```

use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;

/// Top-level shape of `canonical-schemas.toml`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Optional override for the directory containing the JSON Schema
    /// docs. Resolved relative to the TOML file's directory. Defaults
    /// to `"../docs/protocol/schemas"`.
    #[serde(default = "default_schema_root")]
    pub schema_root: String,

    /// One entry per schema to register.
    pub schemas: Vec<SchemaEntry>,
}

/// One row of `[[schemas]]`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SchemaEntry {
    /// Schema name. Stable namespace identifier; e.g.
    /// `themisra.proof-of-prompt`. Bound to `SafeString`'s default
    /// length cap; the chain rejects longer names.
    pub name: String,

    /// Schema version, monotonic per `name`. Different `(name, version)`
    /// pairs produce different `schema_id`s.
    pub version: u32,

    /// JSON Schema file basename, resolved relative to `schema_root`.
    /// The file's bytes are hashed (after canonical-form check) to
    /// produce the on-chain `payload_shape_hash`.
    pub spec_file: String,

    /// M-of-N threshold for the attestor set. Must be `>= 1` and
    /// `<= attestor_members.len()`.
    pub attestor_threshold: u32,

    /// Attestor pubkeys, each as a 64-char lowercase hex string
    /// (32-byte ed25519 public key). Order matters: it's part of the
    /// `attestor_set_id` derivation.
    pub attestor_members: Vec<String>,

    /// Fee routing share in basis points (1/10000). `0` for no routing.
    /// The chain caps this at `attestation::DEFAULT_MAX_BUILDER_BPS`
    /// (5000 = 50%) and rejects values above that.
    #[serde(default)]
    pub fee_routing_bps: u16,

    /// Optional fee-routing destination address (`lig1...`). Required
    /// iff `fee_routing_bps > 0`.
    #[serde(default)]
    pub fee_routing_addr: Option<String>,
}

fn default_schema_root() -> String {
    "../docs/protocol/schemas".to_string()
}

impl Config {
    /// Parse a config from a TOML file on disk and validate the basic
    /// shape (threshold bounds, hex-shaped pubkeys, fee-route consistency).
    /// Does NOT touch the network or read the schema spec files; that's
    /// the registration logic's job.
    pub fn load(path: &Path) -> Result<Self> {
        let raw =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let cfg: Self = toml::from_str(&raw)
            .with_context(|| format!("parsing {} as canonical-schemas.toml", path.display()))?;
        cfg.validate()?;
        Ok(cfg)
    }

    /// Validate the parsed config without I/O.
    pub fn validate(&self) -> Result<()> {
        if self.schemas.is_empty() {
            bail!("[[schemas]] is empty; nothing to register");
        }
        for (i, s) in self.schemas.iter().enumerate() {
            s.validate().with_context(|| format!("schemas[{i}] ({})", s.name))?;
        }
        Ok(())
    }

    /// Resolve [`Self::schema_root`] against the directory of the
    /// originating TOML file.
    pub fn resolve_schema_root(&self, toml_path: &Path) -> PathBuf {
        let parent = toml_path.parent().unwrap_or_else(|| Path::new("."));
        parent.join(&self.schema_root)
    }
}

impl SchemaEntry {
    fn validate(&self) -> Result<()> {
        if self.name.is_empty() {
            bail!("name must not be empty");
        }
        if self.version == 0 {
            bail!("version must be >= 1");
        }
        if self.attestor_members.is_empty() {
            bail!("attestor_members must not be empty");
        }
        if self.attestor_threshold == 0 {
            bail!("attestor_threshold must be >= 1");
        }
        if (self.attestor_threshold as usize) > self.attestor_members.len() {
            bail!(
                "attestor_threshold ({}) exceeds attestor_members.len() ({})",
                self.attestor_threshold,
                self.attestor_members.len()
            );
        }
        for (i, m) in self.attestor_members.iter().enumerate() {
            if m.len() != 64 {
                bail!("attestor_members[{i}] is not 64 hex chars (got {} chars)", m.len());
            }
            hex::decode(m).map_err(|e| anyhow!("attestor_members[{i}] is not valid hex: {e}"))?;
        }
        if self.fee_routing_bps > 0 && self.fee_routing_addr.is_none() {
            bail!("fee_routing_bps > 0 but fee_routing_addr is unset");
        }
        if self.fee_routing_bps == 0 && self.fee_routing_addr.is_some() {
            bail!("fee_routing_addr is set but fee_routing_bps is 0; pick one");
        }
        Ok(())
    }

    /// Decode `attestor_members` into raw 32-byte pubkeys.
    pub fn member_bytes(&self) -> Result<Vec<[u8; 32]>> {
        let mut out = Vec::with_capacity(self.attestor_members.len());
        for (i, m) in self.attestor_members.iter().enumerate() {
            let bytes =
                hex::decode(m).map_err(|e| anyhow!("attestor_members[{i}] hex decode: {e}"))?;
            let arr: [u8; 32] = bytes
                .as_slice()
                .try_into()
                .map_err(|_| anyhow!("attestor_members[{i}] is {} bytes; want 32", bytes.len()))?;
            out.push(arr);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> Result<Config> {
        let cfg: Config = toml::from_str(s)?;
        cfg.validate()?;
        Ok(cfg)
    }

    fn one_member_64() -> &'static str {
        "aabbccddeeff00112233445566778899aabbccddeeff00112233445566778899"
    }

    #[test]
    fn parses_minimal_config() {
        let cfg = parse(&format!(
            r#"
[[schemas]]
name = "themisra.proof-of-prompt"
version = 1
spec_file = "themisra.proof-of-prompt-v1.json"
attestor_threshold = 1
attestor_members = ["{m}"]
"#,
            m = one_member_64()
        ))
        .expect("parse");
        assert_eq!(cfg.schemas.len(), 1);
        assert_eq!(cfg.schemas[0].name, "themisra.proof-of-prompt");
        assert_eq!(cfg.schemas[0].version, 1);
        assert_eq!(cfg.schemas[0].fee_routing_bps, 0);
        assert!(cfg.schemas[0].fee_routing_addr.is_none());
        assert_eq!(cfg.schema_root, "../docs/protocol/schemas");
    }

    #[test]
    fn rejects_empty_schemas_block() {
        let err = parse("schemas = []").unwrap_err();
        assert!(format!("{err:#}").contains("empty"));
    }

    #[test]
    fn rejects_threshold_zero() {
        let err = parse(&format!(
            r#"
[[schemas]]
name = "x"
version = 1
spec_file = "x.json"
attestor_threshold = 0
attestor_members = ["{m}"]
"#,
            m = one_member_64()
        ))
        .unwrap_err();
        assert!(format!("{err:#}").contains("threshold must be >= 1"));
    }

    #[test]
    fn rejects_threshold_exceeds_members() {
        let err = parse(&format!(
            r#"
[[schemas]]
name = "x"
version = 1
spec_file = "x.json"
attestor_threshold = 2
attestor_members = ["{m}"]
"#,
            m = one_member_64()
        ))
        .unwrap_err();
        assert!(format!("{err:#}").contains("exceeds attestor_members.len()"));
    }

    #[test]
    fn rejects_short_pubkey() {
        let err = parse(
            r#"
[[schemas]]
name = "x"
version = 1
spec_file = "x.json"
attestor_threshold = 1
attestor_members = ["aabb"]
"#,
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("not 64 hex chars"));
    }

    #[test]
    fn rejects_fee_bps_without_addr() {
        let err = parse(&format!(
            r#"
[[schemas]]
name = "x"
version = 1
spec_file = "x.json"
attestor_threshold = 1
attestor_members = ["{m}"]
fee_routing_bps = 100
"#,
            m = one_member_64()
        ))
        .unwrap_err();
        assert!(format!("{err:#}").contains("fee_routing_addr"));
    }

    #[test]
    fn rejects_addr_without_fee_bps() {
        let err = parse(&format!(
            r#"
[[schemas]]
name = "x"
version = 1
spec_file = "x.json"
attestor_threshold = 1
attestor_members = ["{m}"]
fee_routing_addr = "lig1xyz"
"#,
            m = one_member_64()
        ))
        .unwrap_err();
        assert!(format!("{err:#}").contains("fee_routing_bps is 0"));
    }

    #[test]
    fn rejects_unknown_field() {
        // `deny_unknown_fields` catches typos.
        let err = parse(
            r#"
[[schemas]]
name = "x"
versionn = 1
spec_file = "x.json"
attestor_threshold = 1
attestor_members = ["aabbccddeeff00112233445566778899aabbccddeeff00112233445566778899"]
"#,
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("versionn"));
    }

    #[test]
    fn member_bytes_decodes_correctly() {
        let cfg = parse(&format!(
            r#"
[[schemas]]
name = "x"
version = 1
spec_file = "x.json"
attestor_threshold = 1
attestor_members = ["{m}"]
"#,
            m = one_member_64()
        ))
        .expect("parse");
        let bytes = cfg.schemas[0].member_bytes().expect("decode");
        assert_eq!(bytes.len(), 1);
        assert_eq!(bytes[0][0], 0xaa);
        assert_eq!(bytes[0][31], 0x99);
    }
}
