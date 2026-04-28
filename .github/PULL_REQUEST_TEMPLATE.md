<!--
Template for pull requests against ligate-io/ligate-chain.

Keep one concern per PR. If you're solving more than one problem,
split the work into separate PRs so reviewers can evaluate each
independently. See CONTRIBUTING.md for the full conventions.
-->

## Summary

<!-- One to three sentences. What does this PR do, and why does it matter? -->

## What lands

<!--
Bulleted list of concrete changes. Reviewers should be able to map
each bullet to a chunk of the diff.

Example:
- New `route_get_schema` handler in `crates/modules/attestation/src/query.rs`
- `ModuleRestApi` derive on `AttestationModule<S>`
- 5 integration tests in `tests/integration.rs` covering happy path + 404 + 400
-->

-

## Why this shape

<!--
The "why", not the "what". The diff covers what changed; this section
covers the rationale, alternatives considered, and any non-obvious
trade-offs. Skip if the PR is a one-liner.
-->

## Out of scope

<!--
What you considered but deliberately did not include in this PR, with
a pointer to where it gets handled. Stops reviewers from asking "why
not also fix X?".
-->

## Test plan

<!--
Checkboxes for each thing you verified. Include both local CI gates
and any manual / integration verification.
-->

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] `cargo test --workspace`
- [ ] `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --document-private-items`
- [ ] `cargo audit`
- [ ] `CI pass` green on the pull request

## Related issues

<!--
Use "closes #N" for issues this PR fully resolves and "refs #N" for
ones it touches but does not close. Helps the project board auto-update.
-->

closes #
