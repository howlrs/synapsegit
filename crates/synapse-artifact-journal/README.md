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

Schema v2 adds two crash-safe publication boundaries. A trusted worker calls
`register_proposal_intent` before Proposal CAS; this stores only a private
server-side intent and hashes the raw request/idempotency key. After the worker
has verified successful Proposal publication, `commit_proposal_publication`
atomically creates the pending review and its public-safe `ReviewId`. Exact
retries return the same private intent or ReviewId. Unfinalized private intents
can be recovered per authorized project with
`list_unfinalized_proposal_intents`; the public Proposal manifest digest linked
to a finalized review is available without exposing its binding.

For a Human Decision, `register_decision_commit_intent` durably binds the full
stored `ReviewBinding`, disposition, selected snapshot, reviewed manifest
digest, new Decision head, and feedback OID. After the application verifies the
real Core receipt, `commit_decision_outcome` requires an exact match for the
Proposal head, expected/new Decision heads, feedback OID, and semantic fields,
then writes the outcome and `decision_committed` state in one SQLite
transaction. `get_review_reconciliation` returns a consistent review, strict
intent, and outcome view after restart. Legacy schema-v1 rows remain readable;
they cannot be promoted into a v2 verified outcome without a new exact intent.
After an upper layer proves from the live Ref/reflog that an unknown CAS did
not commit, `reconcile_decision_not_committed` can move only that exact stored
v2 intent to `retryable_failure`; ordinary state transitions cannot do so.

The journal is not authentication, authorization, a portable permit, or proof
that SynapseGit Core admitted a Proposal or Decision. Callers must authenticate
and authorize a project before lookup, reconstruct trusted application
authority from server-owned configuration, and revalidate immutable objects and
live Refs through the normal Core publication path. In particular, the journal
does not verify a Core receipt: callers must do so before either publication
commit method.

Credentials, permits, Actor/Policy/Grant identifiers, repository paths, raw
idempotency keys, rationale, and canonical request bytes are outside the schema.
