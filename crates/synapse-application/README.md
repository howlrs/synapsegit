# synapse-application

`synapse-application` is the Stage 0 authenticated service-embedding boundary
for Creative AI execution/publication and one narrow Human Decision route. It
keeps credentials, exact project routes and ACLs, trusted authority profiles,
registrations, candidates, the AI executor, the clock, and one-shot permits
outside the untrusted request plane.

This is a synchronous, process-local Rust library, not an HTTP server,
credential or membership database, durable permit service, model sandbox,
connector host, multi-process coordinator, Projection route, organization or
quorum workflow, release approval route, or modified/partial adoption workflow.

## Creative AI route

- Authentication runs before project, execution-handle, permit, or Repository
  lookup. Unknown, malformed, and forbidden project selections share the exact
  `project_access_denied: project access denied` public result. This is a
  semantic anti-oracle, not a constant-time or traffic-analysis guarantee.
- Actor, principal, authority OIDs, base/target Ref names, exact/runtime
  capabilities, clock, and executor come only from trusted control-plane state.
- `prepare_ai` orders authentication, exact project/ACL/registration lookup, the
  project FIFO fence, candidate-independent Core preflight, and atomic exchange
  of the one-time registration for one opaque `AiExecutionPermit`. A failed
  preflight leaves the registration available.
- The AI permit is non-cloneable and process-local. Its exclusive deadline is
  the earlier of application TTL and immutable Grant expiry
  (`now < not_after`).
- `execute_and_publish_ai` authenticates before permit lookup. Authentication
  failure leaves a Ready permit usable. Once an authenticated attempt claims
  the matching registry entry, the permit is irreversibly burned before the
  trusted `AiExecutor` runs.
- The executor runs without the application-state, publication-gate, or
  Repository lock held. The route then reauthenticates, enters the project FIFO
  fence, rechecks live ACL/profile generation and deadline, rebuilds authority,
  and holds the fence through full Core revalidation and Ref transaction.
- Clock failure or reversal, expiry, ACL/profile mutation, executor failure,
  Core denial, `stale_base`, and `ref_conflict` never restore a burned permit.
- Successful publication returns `AiPublicationReceipt`: the Core
  `AuthorizationDecision` plus an opaque `AdmittedProposalHandle` created from
  the committed proposal reflog result.

## Narrow Human Decision route

The Human route accepts only a proposal successfully published by the same
`Application` instance through the AI route.

- `AdmittedProposalHandle` is non-cloneable, redacted in `Debug`, and bound to
  the application instance, exact project, proposal Ref, and committed head. It
  is process-local evidence, not a portable receipt, signature, or post-restart
  proof.
- Trusted control-plane code installs a reusable
  `HumanAuthorityProfileConfig` fixing the project, direct human ID, canonical
  decision Ref, Human Actor OID, and Policy OID. It separately creates a
  server-owned `HumanDecisionCandidate` containing only the new Decision
  Commit, DecisionFeedback OID, and bounded message.
- `register_human_decision` borrows the admitted-proposal handle. Under the
  project FIFO fence it binds the exact proposal, candidate, live profile
  generation, and canonical decision Ref's current head into one non-cloneable
  `RegisteredHumanDecisionHandle`.
- The admitted handle itself remains reusable. After a registration, permit,
  candidate, or Core denial, trusted code may borrow it again to register a
  corrected candidate. Each registration and `HumanDecisionPermit` is one-shot;
  Core duplicate-lineage validation and the canonical decision Ref CAS preserve
  one canonical disposition of the proposal.
- `prepare_human_decision` accepts only credential, exact project selector, and
  opaque registration. After authentication it checks the process ACL, live
  unsuspended profile and registration under the FIFO fence, then issues one
  permit with the exclusive application TTL only.
- `publish_human_decision` authenticates once before permit lookup. Credential
  rejection leaves a Ready permit untouched. After successful authentication
  claims the matching permit entry, every remaining outcome burns it. The route
  enters the same FIFO fence, rechecks deadline, ACL, profile generation and
  suspension, reconstructs `HumanDecisionAuthority`, and holds the fence through
  full `HumanDecisionRuntime::publish_decision` validation and its proposal
  precondition/canonical decision CAS.
- Human publication has no executor, no separate Core preflight, and no second
  authentication. An invalid Human permit reuses
  `execution_permit_invalid: execution permit invalid`.
- A Clock failure while preparing is `service_unavailable`. After a Human permit
  is burned, a pre-Core Clock failure, backward observation, or expiry is
  `execution_permit_invalid`; a deadline/backward failure observed inside final
  Core publication, including its initial trusted-Clock read after static
  validation or its transaction guard, remains Core `storage_error`.

## Authentication and revocation residual

Every Authenticator callback runs outside the project FIFO fence and outside
application-state and Repository locks. Its result is a point-in-time session
decision. The fence linearizes only process-local ACL and authority-profile
changes; it does not instantaneously fence an external credential-store
revocation while a request is queued. Permit TTL bounds this residual window.
Production authentication adapters and credential lease/revocation semantics
remain a deployment responsibility.

## Stable errors and process boundary

Application-layer public errors are `authentication_required`,
`project_access_denied`, `execution_permit_invalid`, `execution_failed`,
`configuration_invalid`, and `service_unavailable`. Detailed Core errors pass
through only from a final authenticated AI or Human publication after the
matching permit has been burned. Candidate-independent AI Core preflight denial
is normalized to `configuration_invalid`; its operational storage/resource
failure becomes `service_unavailable`. The Human route has no separate Core
preflight. Authenticator and executor panics are contained.

Project ACL and AI/Human profile mutation use the same FIFO fence as
publication. The in-memory project map, ACL, profiles, registrations, permits,
and fences last for one process lifetime; restart invalidates every outstanding
handle and permit. Production deployment needs concrete authentication,
durable authenticated control-plane state, and an equivalent fair, linearizable
fence across all service processes. Trusted control-plane methods on
`Application` must never be exposed as untrusted request routes.
