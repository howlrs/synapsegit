# synapse-artifact-journal

`synapse-artifact-journal` is a transport-neutral SQLite journal for local
artifact review workflows. It durably maps a public-safe opaque `ReviewId` to
server-owned project, Proposal, and Decision bindings, records a bounded review
state, and deduplicates one Decision intent with a SHA-256 idempotency digest.
`create_or_get_review` makes response-loss retries return the original locator
for an exact binding. Review state updates are compare-and-set: committed and
terminal-denial states cannot be reopened, while `outcome_unknown` can move only
to a terminal state after external reconciliation. Repeating the same state is
idempotent.

The journal is not authentication, authorization, a portable permit, or proof
that SynapseGit Core admitted a Proposal or Decision. Callers must authenticate
and authorize a project before lookup, reconstruct trusted application
authority from server-owned configuration, and revalidate immutable objects and
live Refs through the normal Core publication path.

Credentials, permits, Actor/Policy/Grant identifiers, repository paths, raw
idempotency keys, rationale, and canonical request bytes are outside the schema.
