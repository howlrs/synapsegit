# Implementation boundaries and library-only features

Audience: implementers, tool builders, and evaluators who embed or review the
Rust workspace. Creators do not need this page to use the packaged binaries.

The root [README](../README.md) describes what the three packaged binaries
(`synapse`, `synapse-local`, `synapse-present`) do for a creator. This page keeps
the technical boundaries of those binaries and the features that exist only as
workspace libraries. It was moved out of the README without changing its
meaning.

“Implemented” means covered by this repository's tests. A library or schema
surface listed here is not a tested transport integration or a packaged
binary feature. Neither label means that real-user authentication, network
transport, production operations, or a general creator-facing application is
ready.

## Packaged binary boundaries

`synapse-local` includes browser import and review, dedicated diagnostics, and
bounded browser `fsck`. Review authority and maintenance job state are
process-local and cannot be resumed after a restart. After a restart, a
verified interrupted proposal can be reviewed in a new session using its
recorded three images; a Defer can use the same fresh-review path. The original
session is never resumed or changed.

`synapse-present` reads the existing CAS without mutation and copies
checkpointed Ref SQLite (up to 512 MiB) into a private temporary file, requiring
the copy-time and post-copy source SHA-256 to match; SQLite never opens the
source database directly. Sidecars or a changing source fail with
`read_only_source_busy`. It discovers at most 100 creator sessions and can
prepare a local GitHub-ready view, but it does not upload, publish, or contact
GitHub. Private rationale, internal Actor IDs, repository paths, and raw assets
stay omitted; raw-asset rendering is not implemented, and a public note is
separate author-supplied text. See the
[CLI reference (Japanese)](./cli_reference.md#synapse-present-cli).

## Generic regular-file artifact libraries

The source and workspace libraries include evaluation-only building blocks for
sibling applications implementing generic regular-file review.
`synapse-artifact` validates a complete regular-file manifest and
deterministically maps it to a nested site Tree without advancing a Ref. Its
trusted workflow initializes a profile-owned repository, publishes at most one
active Proposal from each exact canonical Decision head, and records one
`adopted_unchanged`, `rejected`, or `deferred` Decision through
`synapse-application` and Core. A completed Decision can become the verified
accepted base for the next Proposal; each attempt has a fresh deterministic Ref
and immutable identity, while prior Proposal history remains reachable.

The same-process pending authority remains non-serializable and one-shot.
`decide_artifact_proposal` additionally requires an opaque, expiring
`ArtifactDecisionApproval` issued only after the embedding host authenticates
the reviewer and checks a server-owned project ACL. The approval is bound to
the exact actor/session, security epoch, Proposal and expected Decision heads,
disposition, rationale presence and bytes, and is burned before Decision object
or Ref mutation. It is not reconstructed from browser fields or a `ReviewId`.
The v1 workflow still records caller-supplied AI-attributed bytes only and
cannot represent verified execution; SynapseGit invokes no model.

The frozen
[`synapsegit.generic-artifact` v1 contract](../spec/application/generic-artifact/v1/README.md)
uses an opaque `ReviewId` as a lookup locator, not as authority. A separate
SQLite journal and explicit orchestration boundary register a private Proposal
intent before Proposal CAS, finalize the public-safe locator only after exact
publication verification, persist an exact Decision intent before Decision
CAS, and commit a terminal outcome only after live Ref/reflog reconciliation
and bounded selected-site checkout. Exact retries are idempotent. After a
restart, trusted configuration plus journal facts are checked against immutable
objects and one consistent live Ref state before fresh application authority is
constructed. Credentials, admitted handles, approvals, registrations, and
permits are never serialized or restored; the reviewer must authenticate again
and obtain a new approval, and final publication still passes through ordinary
`HumanDecisionRuntime` validation and CAS.

The binding assumes untampered trusted local configuration and journal storage;
it is not cryptographic evidence that the original Proposal passed a particular
process runtime capability intersection. Core Ref/reflog and journal SQLite
transactions are separate, so crash windows are resolved by explicit bounded
reconciliation rather than by claiming cross-database atomicity. Rust trusted
workflow values are getter-only process values, not browser-supplied authority.

These capabilities are not exposed by any of the three packaged binaries,
including through HTTP, CLI, or browser UI. They also do not provide a
background service that resumes work automatically, model invocation, a generic
browser editor, durable identity or ACL storage, multi-process
linearizability, production use, or a distribution permission. The packaged
Creator flow and localhost UI remain image-specific; their pending review
authority is still same-process and non-resumable.

## Generic-artifact projection and local bundle

The source and workspace libraries also include a versioned generic-artifact
projection and local bundle API. This API is not exposed by the packaged
binaries, HTTP, CLI, or browser UI. A complete projection is built only through
the bounded Decision checkout above; pending/incomplete projections carry no
repository or authority identifiers. Canonical JSON, escaped Markdown,
script-free HTML, manifest, checksums, and local Synapse/GitHub layouts are
generated without Git or network access. Remote Synapse/GitHub adapters, Git
import and provenance, identity mapping, a GitHub App, and a hosted service
remain future implementation work. The design scope of
[#17](https://github.com/howlrs/synapsegit/issues/17) is resolved without making
any of those remote surfaces available.

See the [generic publication profile](../spec/application/generic-artifact-publication/v1/README.md)
and the [integration roadmap](./generic_artifact_publication_roadmap.md).
