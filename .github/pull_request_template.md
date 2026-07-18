## Summary

Describe what changes and which user or implementation problem it solves.

## Invariants and compatibility

- [ ] I identified any effect on canonical bytes, OIDs, schemas, fixtures, Refs, archives, or stable error codes.
- [ ] I updated the normative spec before or with a protocol change.
- [ ] I documented migration or backward-compatibility behavior, or confirmed there is no impact.
- [ ] Evidence, Analysis, Claim, AI Proposal, and Human Decision remain distinct.

## Security, privacy, and Human Gate

- [ ] I reviewed trust-boundary, sensitive-data, and failure-mode changes.
- [ ] AI-controlled input cannot write Human Decision or release history through this change.
- [ ] User-facing text does not claim proof of authorship, truth, copyright, permanence, productivity, or physical change.
- [ ] I updated `SECURITY.md` or the security model when the supported boundary changed.

## Contribution terms

- [ ] I have the legal right to submit this contribution and have disclosed any applicable third-party terms.
- [ ] I have read and agree to the contribution grant in [`LICENSE` Section 5](../LICENSE), including the license to howlrs and K-Terashima; I understand this is not a copyright assignment.

## Verification

List the exact commands run and their results.

```text
cargo fmt --all -- --check
cargo test --workspace --all-targets --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked
node scripts/verify_core_fixtures.mjs
node scripts/verify_local_api.mjs
node scripts/test_publication_comprehension_scorer.mjs
node scripts/verify_license.mjs
node scripts/generate_third_party_notices.mjs --check
node scripts/verify_docs.mjs
node scripts/verify_mermaid.mjs
git diff --check
```

## Documentation and release impact

- [ ] I updated the root README, project status, documentation index, CLI reference, or release notes where applicable.
- [ ] I kept implemented, preview, architecture-only, and planned capabilities explicit.
- [ ] I listed follow-up work that is intentionally outside this PR.

See [CONTRIBUTING.md](../CONTRIBUTING.md) for the complete change-specific checklist.
