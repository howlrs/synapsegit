# Generic-artifact publication roadmap

## Current boundary

The implemented v1 scope is a provider-neutral, local-only, read-only
projection of generic-artifact outcomes. Complete outcomes are derived through
the trusted artifact checkout boundary. Pending and incomplete outcomes are
explicitly non-authoritative display states. The only application input is the
reviewed PublicTarget allowlist, and export produces deterministic canonical
JSON, escaped Markdown, script-free HTML, manifests, checksums, and local
Synapse or GitHub staging layouts.

No current API uploads content, invokes Git, reads a remote repository, maps an
identity, authenticates a GitHub user, installs a GitHub App, or operates a
hosted service. Those capabilities must remain separate from the frozen public
projection and renderer profiles.

## Decisions and priority

The staged order is deliberately narrower than the full integration vision:

1. Keep the implemented deterministic local Synapse/GitHub staging layouts as
   the review boundary. A Human inspects the exact detached bundle before any
   external action.
2. Make a completed, explicitly public GitHub Release the first automated
   write target. A Release can retain one immutable bundle/checksum set without
   turning an Issue body or Discussion index into provenance authority.
3. Add an optional Issue summary only after the Release receipt exists. The
   summary links to the verified digest and must not embed private evidence.
   Discussion synchronization is later work because edit, moderation, and
   reply history need a separate reconciliation contract.
4. Implement local-Git-only read-only import before any GitHub API importer.
   Remote fetch and account association are not implicit import steps.
5. Add consent-based identity linking and hosted multi-user operation only
   after their independent schemas, authorization, revocation, and retention
   designs are accepted.

The minimum external publication payload is the already verified public
bundle, its bundle/projection digests, an explicit source contract/profile
version, destination identity, and an adapter receipt. Raw site bytes and
paths, repository paths, private rationale, prompts, provider responses,
credentials, internal OIDs/Refs, Actor/Policy/Grant records, and unpublished
identity evidence remain excluded. A destination being private is not treated
as sufficient consent; the exact bytes and visibility still require a positive
confirmation immediately before the first write.

The initial importer scope is one caller-selected local repository opened
without hooks, network, submodule initialization, replace refs, or credential
helpers. Identity mapping is a separate optional report keyed by external
evidence, not part of the imported Commit identity. The first GitHub App
adapter requests only metadata read plus the single repository permission
needed by its enabled write target; Release and Issue permissions are never
silently bundled. Installation consent does not license user content, and the
adapter must require the operator to make the applicable repository, content,
and redistribution rights decision outside SynapseGit.

## Remote publication adapter

The adapter is an effectful layer over a verified local bundle, not a feature
of projection construction or verification. Credentials and destination
configuration must stay outside projection JSON and bundle contents.

Acceptance requires:

- explicit destination and account selection, a dry-run destination diff, and
  positive confirmation before the first external mutation;
- a privacy re-verification of the exact bundle bytes immediately before send;
- idempotency keyed by destination plus projection and bundle digests;
- bounded retry behavior and explicit partial, complete, and rollback outcome
  receipts bound to the sent digests;
- least-privilege credentials, redacted diagnostics, and no credential or
  remote locator written into the provider-neutral projection; and
- tests proving that retry, timeout, duplicate delivery, and partial failure do
  not create an authoritative Synapse Decision or silently overwrite remote
  state.

## Local Git importer

The importer is a separate read-only provenance adapter. It must not be added
to the generic-artifact publication profile and must not imply that Git history
is Synapse authority.

Acceptance requires:

- local repositories only, with no fetch, push, credential lookup, hook
  execution, submodule network access, or other network operation;
- bounded traversal and validation of commit DAGs, trees, blobs, author and
  committer fields, timestamps, messages, and supported signatures;
- preservation of external provenance and deterministic, idempotent mapping
  into new Synapse records without rewriting existing records;
- defined behavior for merges, renames, deletions, shallow history, missing
  objects, replace refs, grafts, force-push divergence, and malformed objects;
  and
- no automatic mapping from an email address, signature, or GitHub handle to a
  Synapse Actor.

## Identity mapping

Identity evidence belongs in a versioned, independently reviewed mapping
contract. Unknown or intentionally unlinked identity must remain valid.

Acceptance requires:

- explicit provenance and confidence for self-asserted, provider-linked, and
  Human-confirmed mappings;
- consent, revocation, replacement, collision handling, and historical audit;
- no inference that a matching display name or email proves one person; and
- language and schemas that do not elevate identity mapping into proof of
  authorship, rights ownership, review, or publication authority.

## GitHub App

The GitHub App is an external integration and must consume verified adapter
inputs rather than reaching into private projection sources.

Acceptance requires:

- explicit installation consent and documented minimum permissions;
- secure short-lived token handling, rotation, uninstall, and revocation;
- webhook signature verification, replay protection, deduplication, ordering,
  bounded payloads, and auditable delivery state;
- clear separation of bot actions, GitHub users, and Synapse Human Decisions;
  and
- end-to-end tests for permission loss, repository transfer, deletion,
  suspension, rate limits, and partial publication failure.

## Hosted Synapse service

A hosted service is a separate security and operations project. Its public
views remain derived presentations; they are not the authority for canonical
Synapse records.

Acceptance requires:

- tenant isolation, authentication, authorization, encryption, secret
  management, and immutable security audit trails;
- documented storage, backup, retention, export, account deletion, and
  artifact deletion semantics, including caches and replicas;
- privacy review for indexes, previews, logs, analytics, abuse systems, and
  support access;
- availability, recovery, latency, quota, rate-limit, and incident-response
  objectives; and
- reproducible verification that hosted bytes correspond to a specific local
  bundle digest without claiming that hosting signs or authors the content.

## Versioning rule

Remote receipts, Git provenance, identity evidence, GitHub installation data,
and hosted-service metadata must use new contracts and explicit dispatch. They
must not be inserted into publication v1 fields, change v1 renderer bytes, or
alter the established creator publication v1 schema and comprehension corpus.
