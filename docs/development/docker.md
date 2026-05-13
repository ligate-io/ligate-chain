# Container image: base choice, hardening, threat model

The `ligate-node` container image is built from the repo's [`Dockerfile`](../../Dockerfile) and published to `ghcr.io/ligate-io/ligate-chain` on every `v*` tag via [`docker.yml`](../../.github/workflows/docker.yml). This page covers why the image is shaped the way it is, what threats it does and doesn't defend against, and the operational rules for keeping it healthy over time.

Companion runbook: [`public-devnet-deploy.md`](public-devnet-deploy.md) covers the bare-metal-on-GCP deploy path; this page covers the container variant operators choose when they prefer Docker / Kubernetes / Nomad over a systemd unit.

---

## Base image choice

The runtime stage uses `debian:bookworm-slim` rather than:

| Alternative | Why not (for v0) |
|---|---|
| `gcr.io/distroless/cc-debian12` | Smallest attack surface, but no shell makes operator debugging painful pre-devnet. Worth revisiting once the chain's failure modes are well-understood. |
| `alpine:3.20` | musl libc requires rebuilding `librocksdb-sys` and several `*-sys` deps with `-target=x86_64-unknown-linux-musl`. The toolchain story is doable but added friction; debian-slim is the conservative choice. |
| `ubuntu:24.04-minimal` | Heavier than debian-slim (~80 MB vs ~30 MB compressed) for no security gain since both pull from the same Debian CVE feed. |
| `scratch` (binary-only) | The binary links libgcc + libstdc++ (via `librocksdb-sys`) and TLS roots, all of which need to be present. Plus no `/etc/passwd` for the non-root user. Not viable without a static-musl rebuild. |

Debian-slim wins on the "boring, well-understood, minimal" axis. Switch to distroless once we have a deploy story that doesn't depend on operator-side shell access (i.e., once `/health` + `/ready` + structured logs cover the failure-mode investigation needs that `docker exec sh` currently fills).

---

## Hardening choices

### Multi-stage build

The builder stage (`rust:1.93-bookworm`) carries the full Rust toolchain, `libclang`, `build-essential`, and all the `*-sys` build deps the chain transitively pulls in. None of that lands in the runtime image: only the stripped `ligate-node` binary plus the three runtime libraries (`ca-certificates`, `libgcc-s1`, `libstdc++6`) do.

Net effect: the published image is ~30 MB compressed, ~80 MB uncompressed. The builder image (~2 GB) never leaves CI.

### Non-root user

The runtime stage creates a `ligate` system user with UID 1000 and no login shell (`/sbin/nologin`). `USER ligate` switches the container to that user before `ENTRYPOINT` runs. The binary's working directory `/var/lib/ligate` is owned by `ligate:ligate`.

Why it matters: a remote-code-execution bug in `ligate-node` that lets an attacker run arbitrary code inside the container can't escalate to root inside the container. Combined with the kernel's user-namespace isolation, this means the attacker also can't escalate to root on the host through standard exploit paths. (Container escape via kernel CVE remains in scope; that's a host-kernel patching concern, not an image hardening one.)

### Pinned base digests

Both base images are pinned by `@sha256:` digest, not by tag. The Docker `FROM rust:1.93-bookworm` line resolves to a different image six months from now (CVE fixes, base-image rebuilds, `apt` archive shifts); the digest pin makes the build byte-reproducible across time.

Current pins:

```Dockerfile
FROM rust:1.93-bookworm@sha256:7c4ae649... AS builder
FROM debian:bookworm-slim@sha256:67b30a61... AS runtime
```

**Bump cadence:** monthly review (cron-scheduled scan in [`docker-scan.yml`](../../.github/workflows/docker-scan.yml) catches new CVEs against the pinned base; manual bump PR follows). Bumping is a single-line edit in the Dockerfile + a CI run to confirm nothing breaks. Document each bump's "why" in the PR body.

### No build secrets at runtime

The builder stage uses `--locked` against `Cargo.lock` and reads no secrets from the build environment. The runtime stage carries the binary only; no `--mount=type=secret` paths, no environment-baked tokens. Operator-provided secrets (`SOV_CELESTIA_RPC_AUTH_TOKEN`, `SOV_CELESTIA_SIGNER_KEY`, etc.) come in at runtime via env vars or volume-mounted config, never baked into the image.

---

## CI scanning

[`.github/workflows/docker-scan.yml`](../../.github/workflows/docker-scan.yml) runs [Trivy](https://github.com/aquasecurity/trivy-action) against the image on three triggers:

1. **PR touching `Dockerfile`** — gates on HIGH/CRITICAL CVEs. A PR that adds a new system package introducing a known CVE fails the check.
2. **Push to `main`** (informational, non-gating) — uploads findings to the GitHub Security tab so the team has visibility on new disclosures against unchanged code.
3. **Weekly cron** (Mondays 09:00 UTC) — same informational scan. Catches CVE disclosures against the pinned base that have happened since the last PR.

Findings appear at [`Security → Code scanning alerts`](https://github.com/ligate-io/ligate-chain/security/code-scanning) filtered by `trivy-container`.

`severity: HIGH,CRITICAL` is the gate threshold. MEDIUM/LOW are scanned but don't fail PRs; operators can review the SARIF for context. `ignore-unfixed: false` means CVEs without an upstream patch still surface — useful for understanding the residual risk.

---

## Threat model

What this image is designed to defend against:

- **Operator running an untrusted contributor's PR locally.** The non-root user + minimal runtime stage limits blast radius even if a malicious build script slipped through. (The Rust toolchain's `build.rs` runs at builder-stage time, in CI — still concerning, but separate from the runtime image's design.)
- **A vulnerability in a transitive dep that becomes RCE.** Non-root limits in-container privilege; minimal runtime image limits the post-exploit attack surface (no shell, no compiler, no curl).
- **CVE in a system library shipped with the base.** Pinned digests + weekly scans + Trivy PR gate catch this. Disclosure-to-detection window is at most one week + one bump-PR cycle.

What this image deliberately doesn't defend against:

- **Container escape via host kernel CVE.** Operator hardens the host OS (kernel patching, AppArmor / SELinux profiles, seccomp filters); we can't ship that in the image.
- **Compromised Docker Hub mirror.** Mitigated by digest pinning. A man-in-the-middle attack on Docker Hub that swaps the image at the tag level would fail the digest check; an attack on the underlying registry's content store is a vendor-side concern.
- **Operator running with `--privileged` or mounting the docker socket.** Both of those neutralize container isolation. Document in the deploy runbook ("never run `ligate-node` with `--privileged`"); we can't prevent it at the image level.
- **Supply-chain attack on a Cargo dep.** Mitigated by `cargo audit` (in `ci.yml`) and the pinned `Cargo.lock`. Out of scope for this page; see [`upgrades.md`](../protocol/upgrades.md) for the broader story.

---

## When to bump the base digest

Three triggers:

1. **Weekly cron flags a HIGH/CRITICAL CVE on the pinned base.** Bump to the latest digest of the same major.
2. **Rust toolchain version moves.** If `rust-toolchain.toml` bumps to 1.94, the builder digest follows.
3. **Quarterly hygiene.** Even without a flagged CVE, the base accumulates rebuilds for non-flagged fixes. A quarterly bump catches those without waiting for a CVE forcing function.

Procedure:

```sh
# 1. Find the current digest for each tag
curl -sL "https://hub.docker.com/v2/repositories/library/rust/tags/1.93-bookworm/" \
    | jq -r '.digest'
curl -sL "https://hub.docker.com/v2/repositories/library/debian/tags/bookworm-slim/" \
    | jq -r '.digest'

# 2. Update the two `@sha256:` lines in `Dockerfile`

# 3. Open PR. CI's `docker-scan` job builds + scans the new image
#    on PR; merge if green, revisit if HIGH/CRITICAL fires.
```

The bump PR's body should note which CVE feed triggered the bump (or "quarterly hygiene") so the audit trail is clear.

---

## Related

- [`Dockerfile`](../../Dockerfile) — the artifact this doc explains
- [`.github/workflows/docker.yml`](../../.github/workflows/docker.yml) — publishes the image on `v*` tags
- [`.github/workflows/docker-scan.yml`](../../.github/workflows/docker-scan.yml) — Trivy gates + watchdog
- [`public-devnet-deploy.md`](public-devnet-deploy.md) — bare-metal-on-GCP deploy (the alternative to containers)
- [Issue #195](https://github.com/ligate-io/ligate-chain/issues/195) — original Dockerfile (closed)
- [Issue #280](https://github.com/ligate-io/ligate-chain/issues/280) — this hardening pass (closes when this lands)
