# GitHub repository security baseline

This directory records the owner-applied GitHub settings that cannot be
enforced by repository contents alone. The machine-readable source is
[`security-baseline.json`](./security-baseline.json).

CI and tagged releases run the offline schema check below. It performs no
network request and does not inspect or mutate live settings:

```console
node scripts/manage_github_security.mjs --validate
```

Run the read-only drift check from the repository root with an authenticated
GitHub CLI session:

```console
node scripts/manage_github_security.mjs --check
```

An owner may reconcile the exact recorded baseline with:

```console
node scripts/manage_github_security.mjs --apply
```

`--apply` is deliberately fixed to `howlrs/synapsegit`. It enables Dependabot
alerts and security updates, automated security fixes, secret scanning, push
protection, and private vulnerability reporting. It also makes squash the only
merge method, deletes merged branches, and creates or updates two rulesets:

- `Protect main` requires a pull request, the GitHub Actions check named
  `Rust, protocol, and documentation`, a current base, resolved review threads,
  linear history, and no force-push or deletion. The approval count is zero
  because this is currently a single-maintainer repository and self-merge is
  intentional; independent agent review is an operational gate, not a GitHub
  identity approval. With no bypass actor, owner recovery from an Actions
  outage requires temporarily disabling the ruleset in Settings or through the
  API, recording that exception, and reapplying this baseline afterward.
- `Protect release tags` prevents deletion or replacement of `v*` tags. It
  does not block creation, so the release workflow can publish a new version
  after its source commit has passed the protected-main check.

The manager never deletes an unexpected ruleset or changes collaborators,
visibility, Actions permissions, environments, secrets, or releases. It aborts
if an unexpected repository ruleset exists or a baseline ruleset name is
duplicated. Review the JSON and the script before every material policy change.

The baseline complements, rather than replaces, [SECURITY.md](../../SECURITY.md)
and the repository's private vulnerability reporting channel.
