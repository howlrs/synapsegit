# synapse-artifact

`synapse-artifact` is the provider-neutral regular-file mapping and bounded
trusted review boundary for applications that store an artifact tree in
SynapseGit Core. It supports both opaque process-local workflow handles and
authenticated restart reconciliation through the separate durable journal.

The crate accepts in-memory manifests selected by a trusted server. It does not
open caller-selected host paths or follow symlinks. All manifest validation
completes before the first CAS write.

The v1 mapper:

- accepts regular-file entries only;
- requires relative NFC paths with portable `/` separators;
- rejects traversal, duplicate, lowercase, file/directory, Windows reserved-name,
  reserved-character, trailing-dot/space, and bidi-control conflicts;
- enforces file, byte, depth, and path limits before CAS mutation; and
- maps the same normalized paths and bytes to the same nested ManifestTree OID.

The workflow supports an initial Proposal and sequential Proposals after each
completed Decision. Every review receives a server-derived attempt Ref of the
form `proposal/artifact/<project>/<attempt>`. The attempt is derived from the
exact expected Decision head, so concurrent candidates for one base contend on
the same create-only Ref CAS; only one can become active. Proposal history Refs
are retained, and the next accepted manifest must byte-for-byte match the
bounded checkout selected by the current Decision.

Proposal publication is staged:

- `prepare_artifact_proposal` prepares the initial immutable objects;
- `prepare_next_artifact_proposal_at_head` prepares a sequential Proposal only
  for an exact trusted Decision head;
- `publish_prepared_artifact_proposal` consumes the opaque, non-serializable
  handle and performs the ordinary `synapse-application`/Core CAS; and
- `begin_artifact_proposal` and `begin_next_artifact_proposal` remain
  convenience prepare-plus-publish wrappers.

Human Decisions have the same separation. `prepare_artifact_decision` first
claims an approval, performs live Ref preflight, writes immutable Feedback and
Decision objects, and returns an opaque one-shot handle.
`publish_prepared_artifact_decision` alone can consume that handle and publish
through `HumanDecisionRuntime`; `decide_artifact_proposal` is the compatibility
wrapper.

Trusted workflow timestamps use exactly
`YYYY-MM-DDTHH:mm:ss.nnnnnnnnnZ`. Proposal preparation validates both
`recorded_at` and `grant_expires_at`, including the Gregorian calendar and the
requirement that expiry is not earlier than recording, before it opens the
repository or writes CAS data. The existing raw-string configuration
constructor remains available; `try_new` validates raw timestamp strings
immediately without opening the repository, while
`new_with_canonical_timestamps` accepts values that have already crossed the
typed lexical validation boundary.

Canonical application scalars can be built without floating-point conversion:

```rust
# fn example() -> Result<(), Box<dyn std::error::Error>> {
use synapse_artifact::{CanonicalTimestamp, ScaledInteger, Unit};

let exact_second = CanonicalTimestamp::from_unix_nanos(0)?;
assert_eq!(exact_second.to_string(), "1970-01-01T00:00:00.000000000Z");
let trailing_zeroes = CanonicalTimestamp::from_unix_nanos(120_000_000)?;
assert_eq!(
    trailing_zeroes.to_string(),
    "1970-01-01T00:00:00.120000000Z"
);

let width = ScaledInteger::from_decimal_str("1440.0", Unit::Px)?;
assert_eq!(width.mantissa(), "144");
assert_eq!(width.scale(), 1);

let confidence = ScaledInteger::from_decimal_str("0.9200", Unit::Ratio)?;
assert_eq!(confidence.mantissa(), "92");
assert_eq!(confidence.scale(), -2);
# Ok(())
# }
```

`ScaledInteger::from_decimal_str` accepts only a plain exact decimal string.
Exponent notation, `NaN`, infinities, leading `+`, whitespace, and negative
zero are rejected. There is intentionally no `f64` construction path.

The durable orchestration API journals Proposal intent before CAS, reconciles
live Refs after ambiguous outcomes, and journals Decision intent/outcome around
the staged publication calls. A pre-CAS restart deterministically rebuilds the
same immutable Proposal from trusted replay inputs. After Proposal CAS,
`recover_published_artifact_proposal` accepts only trusted configuration, the
exact durable binding, and the journaled manifest digest; it derives and
revalidates the Proposal, Context, Activity, snapshots, authority, and artifact
bytes from the repository before registering fresh process-local Human review
authority. It never restores an old permit or `AdmittedProposalHandle`.

`checkout_artifact_decision` opens the source repository through a stable
read-only snapshot and returns a whole digest-checked regular-file result. Only
the selected snapshot's direct `site` Tree is recursively materialized.
Protected `base`/`control` Trees and their fixed metadata Records are inspected
only to prove exact authority lineage; private control Blob bytes are never
read or returned. Blob reads apply the tighter per-file and remaining-total
limit before allocation and verification.

This crate does not invoke a model or provide HTTP/CLI/UI transport. The v1
workflow accepts only caller-supplied AI-attributed bytes and cannot represent
verified execution. A future application-integrated executor requires a
separately negotiated contract version.

`decide_artifact_proposal` requires an `ArtifactDecisionApproval` issued by an
`ArtifactApprovalRegistry` backed by the embedding host's `Authenticator` and
trusted clock. The registry authenticates before approval lookup, checks its
server-configured project ACL, and binds the opaque one-shot approval to the
host actor/session, project security epoch, exact Proposal and expected
Decision heads, complete Decision intent, and expiry. Claim happens before
candidate CAS writes; a second authentication, stale Ref preflight, and the
ordinary one-shot Application/HumanDecisionRuntime path still follow. Public
review fields, disposition, or idempotency keys are not approval authority.

Approval handles and credentials are process-local and non-serializable. A
restart never restores them: durable orchestration must authenticate again,
reconcile the live Proposal/Decision binding, and issue a new approval. The
registry is transport-neutral and does not provide an HTTP framework or an
identity provider.

`ArtifactSourceAttribution::CallerSuppliedAiAttributed` must never be presented
as verified model execution. The mapper itself makes no claim about how bytes
were produced and never updates a Ref.
