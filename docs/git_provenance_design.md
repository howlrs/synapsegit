# Local Git provenance and identity design

Status: design only, not a supported import format or command.
Scope: the local Git history and identity portion of
[Issue #17](https://github.com/howlrs/synapsegit/issues/17).

This design extends the [publication roadmap](./generic_artifact_publication_roadmap.md).
It keeps Git observations separate from Synapse authority. No current binary
accepts the envelopes described here. Before implementation, the names, strict
JSON schemas, canonical vectors, and storage extension must be reviewed and
registered together under a new application profile; publication v1 stays
unchanged.

## First importer boundary

The first importer reads one explicitly selected local Git repository. It
does not clone, fetch, push, initialize submodules, run hooks or filters, use
credential helpers, or resolve GitHub accounts. Repository configuration and
commit content are untrusted input. Reads must disable replace refs and lazy
fetch, reject grafts, and use literal object IDs rather than revision
expressions supplied by a caller. A worktree checkout is never needed.

Discovery pins the selected Ref names and tips once. Traversal is over those
exact object IDs; a later Ref change produces a separate observation. Raw
object bytes, type, size, and Git object hash are verified before an import
manifest can be accepted. A disappearing or missing object aborts that import;
the importer must not repair it with network access. SHA-1 and SHA-256 Git
repositories have distinct algorithm tags and length checks. Git IDs are
external identifiers, never parsed as Synapse OIDs.

The initial complete-import profile rejects shallow repositories, missing
promisor objects, grafts, malformed objects, cycles, and unsupported object
algorithms. It reports which restriction stopped the import without emitting
a successful partial result. Limits on objects, total bytes, individual object
bytes, depth, Ref count, identity bytes, and message bytes are fixed by the
host before traversal and checked while streaming. The schema/implementation
PR must freeze numeric defaults and hard ceilings with over-limit fixtures.
No unlimited mode belongs in the initial adapter.

## Import envelope

The proposed private envelope has an explicit profile name/version and rejects
unknown fields. Its canonical representation has these logical sections:

| Section | Required meaning |
| --- | --- |
| `source` | Caller-assigned private source ID and Git object algorithm; no embedded repository path, remote URL, or credential |
| `snapshot` | Sorted selected Ref names and exact tips, host observation ordering time and time basis |
| `objects` | Deterministically sorted Git object IDs, types, verified byte lengths, and Synapse Blob bindings to the exact original bytes |
| `commits` | Git ID, ordered parent IDs, root tree ID, exact author/committer header bytes and message bytes through Blob bindings |
| `completeness` | Complete for the selected closure and configured profile, never a claim about every historical Ref or deleted branch |
| `importer` | Adapter/profile versions and the enforced limit profile |

Git text and paths need not be UTF-8. The exact bytes remain in private Blobs;
display strings are explicitly lossy derived values, never identity keys.
Git timestamps and offsets are retained as recorded assertions, distinct from
the host's observation time. A valid timestamp is not proof of when work was
performed. Tree entries preserve modes and raw path bytes. A gitlink records
an external commit ID without following or claiming completeness of the
submodule; symbolic-link content is stored as bytes and never followed.

Merge parent order is preserved. A deletion is the absence of an entry in the
next exact tree; a rename is not inferred as a fact because Git trees do not
record rename intent. Force-push, Ref deletion, and divergent tips produce new
snapshot observations without erasing earlier imported objects or observations.

Object deduplication uses `(algorithm, git_object_id)` after checking the exact
bytes. Equal IDs with unequal bytes fail as an integrity conflict. Repeating
the same selected closure reuses immutable content; a new observation time
can still create a distinct snapshot observation. Ref observations, identity
mapping, and signature assessments do not change imported object identity.
Import publication must be atomic at its application manifest boundary:
failure may leave unreachable verified Blobs but no discoverable successful
import or Human Decision. Retry reuses those verified bytes.

## Identity evidence and confirmation

Git author and committer are separate asserted identities even when their
display strings match. Import never creates a trusted Human actor from an
email, a signature, or a GitHub login. Identity evidence is a separate private,
versioned append-only mapping envelope:

| Field | Design |
| --- | --- |
| `subject` | Source ID, Git commit ID and `author` or `committer` role, bound to the exact original header bytes |
| `candidate` | Optional Synapse actor reference or provider account `(provider, numeric_id)`; provider login is a mutable display value |
| `basis` | One of `self_asserted`, `provider_linked`, `human_confirmed`; these describe evidence origin, not a numerical confidence score or ranking |
| `evidence` | Exact private evidence Blob binding, observer, ordering time/time basis, and provider/API version when applicable |
| `supersedes` | Optional earlier mapping envelope ID, preserving replacement, dispute, unlinking, and revocation history |
| `status` | `proposed`, `confirmed`, `disputed`, or `revoked`; confirmation records the authenticated operator and the exact candidate/evidence approved |

No candidate is a valid unresolved state, not a broken import. Several
candidates remain separate proposals until a Human explicitly resolves them;
the importer never picks one by matching email or display name. A confirmation
is scoped to the displayed evidence/subject. It must not silently map all
commits with the same email or merge two existing Synapse actors. New evidence
does not silently upgrade, restore, or broaden a revoked mapping.

Local import starts with signature status `not_checked`. Later signature
verification is a separate bounded assessment with verifier version, checked
bytes, key fingerprint, trust policy, observation time, and a result such as
`valid`, `invalid`, `unsupported`, or `unavailable`. It must disable automatic
key retrieval and arbitrary repository-configured verification commands.
Cryptographic validity, key trust, provider account association, and Human
confirmation remain separate; none establishes authorship or rights.

## Review and public disclosure

A local preview shows selected tips, counts, limit/completeness checks, and
unresolved or conflicting identity proposals before import publication. The
import operation cannot call Human Decision admission or advance existing
Creator Proposal/Decision Refs. No imported data automatically enters public
Creator or generic-artifact publication v1.

Future provider lookup requires separate consent and an allowlisted account
association response. Publication requires a separately reviewed public
projection: raw Git author/email, message, path, signature, account association,
and mapping evidence stay private by default. Installation consent is not
consent to publish those fields. Revocation prevents subsequent authorized
mapping use; it cannot promise removal of copies already disclosed externally.

## Implementation and acceptance sequence

1. Freeze strict schemas and canonical positive/negative vectors for the
   import and mapping envelopes. Define profile limits and the private storage
   extension; reject unknown versions without changing Core or publication v1.
2. Build a local read-only traversal adapter against synthetic SHA-1 and
   SHA-256 repositories. Verify merges, non-UTF-8 names, symlinks, gitlinks,
   ref changes, repeated import, object corruption, missing objects, shallow
   history, replace refs, grafts, lazy-fetch refusal, and each resource limit.
   Prove zero network, hook, credential, filter, and source-write operations.
3. Add a private preview and atomic import-manifest publication. Exercise
   interrupted reads, interrupted writes, retry, same-ID byte conflicts, and
   no successful partial import. Archive round trips retain exact evidence.
4. Add explicit identity proposal/confirmation/revocation with actor and
   evidence binding tests, including collisions and unresolved identities.
5. Only then add separately consented provider lookup, remote publication,
   and GitHub App delivery using the roadmap's independent security gates.

Each step needs its own implementation PR and execution evidence. Approving
this design neither implements these steps nor authorizes external delivery.
