# Publication comprehension corpus v1

This frozen corpus evaluates whether a reader outside the source author can
understand a SynapseGit publication bundle without access to the Core
repository. It is evaluation material, not proof that Human or AI
comprehension, accessibility, authorship, truth, or publication permission has
already been established.

## Cases

The cases are intentionally separate:

- [`complete`](./bundles/complete/story.md) contains one `adopt`, one `reject`,
  and one `defer` Human decision. It tests role separation, selection, retained
  alternatives, attribution scope, and byte-identity limitations.
- [`incomplete-only`](./bundles/incomplete-only/story.md) contains one pending
  creator session and no complete report. It tests whether readers notice that
  the reachable CAS lineage remains unverified.

Do not merge these cases for evaluation. A mixed bundle with any complete
report uses the complete-report verification scope and therefore masks the
incomplete-only boundary this corpus is meant to expose.

In the incomplete case, `proposal_present=true` and `decision_present=true`
describe observed Ref shapes. They do **not** prove that a Human decision was
completed. The Decision Ref still points to a pre-decision base Commit. The
scored questions use only meanings visible in the selected track: `I04` and
`I05` test whether the session is kept outside the complete-session list,
`I06` tests the JSON-only verification scope, and `I07` tests the unavailable
projection fingerprint.

## Evaluation files

- [`questionnaire.json`](./questionnaire.json) is the only evaluation metadata
  shown to an evaluator, filtered to the selected case.
- [`oracle.json`](./oracle.json) pins each projection digest and provides the
  machine-readable answer key. Never include it in evaluator context.
- [`protocol.json`](./protocol.json) fixes case isolation, run counts,
  thresholds, and accessibility limits.
- [`response.schema.json`](./response.schema.json) defines the response
  envelope for Human and zero-context AI runs.
- [`result-template.json`](./result-template.json) begins at `not_run`; it is
  a publication-summary template, not evidence of an evaluation result. The
  scorer emits a distinct
  `org.synapsegit.publication-comprehension-score-report` document.
- [`privacy-canaries.json`](./privacy-canaries.json) lists synthetic values that
  must remain outside each bundle, plus deliberately public positive controls.

The files under `bundles/` are complete, checksum-covered local Synapse target
bundles. Evaluation metadata remains outside their fixed inventories.

## Run the repository checks

```bash
cargo test -p synapse-publication --test evaluation_corpus --locked
```

The integration test runs the production `verify_bundle` path, checks the
pinned projection digests and semantic oracle, scans literal and Base64 privacy
canaries, and enforces a static HTML/navigation baseline. Static inspection is
not an axe, screen-reader, keyboard, zoom, mobile-device, or WCAG conformance
result; those remain explicit external gates in `protocol.json`.

The dependency-free scorer has its own focused test:

```bash
node scripts/test_publication_comprehension_scorer.mjs
```

## Run an evaluation

Use a fresh evaluator context for every case, track, and run. The allowed
combinations are AI/JSON, AI/HTML, and Human/HTML:

1. For an AI/JSON run, provide the exact bytes of that case's
   `projection.json` and questions whose `cases` and `tracks` both apply.
2. For an AI/HTML run, provide the exact bytes of that case's `index.html` and
   its applicable questions. Do not provide or fetch linked files.
3. For a Human/HTML run, render that case's `index.html` locally in an isolated
   browser and provide its applicable questions. Keep the link visible, but
   instruct the participant not to activate it and prevent navigation during
   the scored run.
4. Never provide this README, the sibling case, oracle, canaries, or previous
   responses.
5. Record the SHA-256 of the input artifact and the non-personal evaluator
   configuration required by `response.schema.json`.
6. Store answers in the response envelope and score all collected responses:

   ```bash
   node scripts/score_publication_comprehension.mjs responses/*.json
   ```

7. Keep the overall status `not_run` until the minimum runs and every critical
   gate in `protocol.json` have actually been completed.

Answers use strict JSON type and value equality. Thresholds use integer cross
multiplication without percentage rounding. Each AI run must pass on its own;
Human macro and per-critical-question gates aggregate distinct participants
within one case on the HTML track. The scorer is the executable definition of
these rules.

Human evaluation must not collect names, email addresses, credentials, or
other unnecessary personal data. Use an opaque run ID; browser metadata must
not identify the participant. Model configuration, notes, and run IDs must not
contain credentials, local paths, or personal identifiers. The committed
corpus contains only synthetic source identities and canaries.

## Freeze and refresh policy

The v1 bundles are golden artifacts. Normal CI verifies them but never
regenerates them. Creator fixture construction uses OS randomness and the system
clock, so a fresh source repository has a new legitimate identity and different
OIDs even when its evaluation meaning is the same.

To create a new candidate in a directory that does not yet exist:

```bash
cargo run -p synapse-publication --example generate_evaluation_corpus -- \
  /tmp/synapsegit-publication-corpus-candidate
```

Recompute and review the candidate's semantic oracle and privacy controls
before accepting new digests. Do not commit the source CAS, Ref database,
private rationale, or raw input assets. Renderer profile v1 is frozen; a
semantic renderer change requires an explicit new profile and corpus version
rather than silently rewriting this corpus.

The production boundary and bundle format are documented in the
[`synapse-publication` README](../../../../crates/synapse-publication/README.md).
