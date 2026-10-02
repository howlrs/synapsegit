# 2026-10-02 multi-AI mocked-environment operational evaluation

Status: executed and reviewed; accepted for the AI mock release gate

Release under test: locally packaged `v1.0.0` candidate, source commit `93a29b770655b9926b0be5e1a6c0941d4a079d9c`

Release-gate scope: [v1.0 release plan](../../../../v1_release_plan.md#リリース条件) / [#192](https://github.com/howlrs/synapsegit/issues/192)

## Purpose and boundary

This record is the v1.0.0 release-gate evidence for three to five operational
trials in isolated AI-mocked environments. Codex, Gemini, and Claude perform
the trial roles. It is not a real-user study: it records no human participants or consent and establishes no human UX
findings or authentic Human Decisions. Normal Decision objects stored by
these scenarios are explicitly marked synthetic test choices; they must never
be reported as a person's decisions.

The helper runs cached `synapse` and `synapse-local` binaries for the Linux
x86_64 `v1.0.0` candidate. They were verified byte-for-byte identical to the
freshly packaged candidate from the source commit above; the helper does not
run directly from an extracted archive. It is not the published
`v1.0.0-rc.1` archive. Locale, viewport, filesystem state, and restart state
are mocked where listed below. These scenarios do not demonstrate native macOS
behavior.

## Required evidence for every scenario

- provider and operator role;
- isolated workspace and the mocked locale, viewport, filesystem, and restart
  conditions;
- commands or automation entry point, exit status, and retained output paths;
- artifact and record checks after each state-changing operation;
- a localhost UI screenshot and an explicit visual-inspection finding;
- S1, S2, and S3 status with links to the evidence; and
- reviewer disposition.

S1 is loss or corruption of a record. S2 is an AI preparation action recording
a Decision, or a mock test value being presented as a Human Decision. S3
is inability to complete Inbox write, import, the UI decision flow, read-back,
or backup. `not yet evaluated` is not a passing result.

## Public evidence files

The complete public layout is four synthetic-material screenshots (`M1.png`,
`M2.png`, `M3.png`, `M4.png`) and one structured `results.json` under
[`artifacts/`](./artifacts/). Keep the visible `MOCK` label, synthetic creator
labels, and fixed mock rationale in the screenshots so their status is
auditable. Do not add real-person metadata, real private records, raw agent
logs or transcripts, shell history, private filesystem paths, real decision
rationale, prompts, or original assets. `results.json` must validate against
[`results.schema.json`](./artifacts/results.schema.json), identify the exact
source commit and binary-identity check above, and link each scenario only to
its synthetic screenshot.

The public normalized measurements are in
[artifacts/results.json](./artifacts/results.json). The reproducible synthetic
exercise is [scripts/browser/run-mock-evaluation.mjs](../../../../../scripts/browser/run-mock-evaluation.mjs).
It creates the mock workspace, not a real-user study.

## Scenarios

| ID | Provider / operator | Mocked conditions | Operational flow | S1 | S2 | S3 | Visual inspection | Status |
|---|---|---|---|---|---|---|---|---|
| M1 | Codex | ja, 1440×1000, isolated synthetic workspace | Inbox → UI review/comparison → mock Adopt → report → export/restore | pass | pass | pass | pass | accepted |
| M2 | Codex | en, 390×844, fresh restore target; no explicit server restart | Inbox → UI review/comparison → mock Defer → report → export/restore | pass | pass | pass | pass | accepted |
| M3 | Gemini | ja, 1440×1000, isolated synthetic workspace | Inbox → UI review/comparison → mock Adopt → report → export/restore | pass | pass | pass | pass | accepted |
| M4 | Claude | en, 1440×1000, isolated synthetic workspace | Inbox → UI review/comparison → mock Adopt → report → export/restore | pass | pass | pass | pass | accepted |

## Per-scenario evidence

### M1 — Codex

- Eight operations passed. Before mock Adopt, creator-report JSON exited 1;
  source and restored fsck each reported objects=24, verified=24, closures=2,
  issues=0; the restored report exactly matched the source report.
- [Synthetic report screenshot](./artifacts/M1.png) shows Japanese desktop
  review, Mock codex, the visible MOCK ONLY rationale, image cards, and a
  completed record. Screenshot SHA-256:
  a6695222a78a14593addd79e45bf539d03e0868ca6346543da292a361ab1b61a.
  Report SHA-256:
  e5bbf630882d9221b8bdb126a60c606041203d70f9668484880244874f382b2c.
- Operator visual inspection passed. S1/S2/S3 passed for this mock flow.
  Reviewer disposition: accepted.

### M2 — Codex

- Eight operations passed under the English mobile viewport. This run restored
  the archive into a fresh target; it did not exercise an explicit server
  restart. The same before-report exit, clean source and restored fsck, and
  exact restored-report comparison as M1 were observed.
- The first automation attempts used incorrect English field/button labels
  (`Inbox session name` and `Expand and compare images`). The actual labels
  are `Inbox session` and `Compare images at full size`; corrected retries
  completed the flow. Language cookies worked correctly. This is a
  test-selector correction, not a product failure.
- [Synthetic report screenshot](./artifacts/M2.png) visibly retains Mock
  codex, the MOCK ONLY rationale, Defer result, and mobile image cards.
  Screenshot SHA-256:
  ff8754f23453707dd4b7ea38c01300f676457649aaf69f8672bdfd7409498d44.
  Report SHA-256:
  12d38ad3bd8e1d6eb2ddeae84bf5d06a8338fdf4cb755495787aad2e5b83946e.
- Operator visual inspection passed. S1/S2/S3 passed for this mock flow.
  Reviewer disposition: accepted.

### M3 — Gemini

- Eight operations passed. The requested Gemini CLI model was gemini-3.8-flash;
  provider usage reported gemini-3.5-flash, which is the actual model recorded
  for this run. Before mock Adopt, the report exited 1; source and restored
  fsck were clean and the reports matched exactly.
- The initial final-text summary emitted zeros instead of a usable answer.
  A second native read of the final report PNG and JSON confirmed the final
  state with ordinary visual findings. The original measured values in
  results.json were unchanged.
- [Synthetic report screenshot](./artifacts/M3.png) shows Japanese desktop
  review, Mock gemini, the visible mock rationale, completed Adopt record,
  image cards, and the file-content check. Screenshot SHA-256:
  553ee50d88d8fc3c9b2f47dea3e3d57ec575f463096447286bf3fd1aa19eeaeb.
  Report SHA-256:
  792e17879319a630ecc79a31802770b744beb63d335503e40125f3e388560f9e.
- Operator visual inspection passed. S1/S2/S3 passed for this mock flow.
  Reviewer disposition: accepted.

### M4 — Claude

- Eight operations passed under the English desktop viewport, using Claude
  Opus 5.5. Before mock Adopt, the report exited 1; both fsck runs were clean
  and the restored report exactly matched the source report.
- [Synthetic report screenshot](./artifacts/M4.png) shows the completed mock
  Adopt record. Operator inspection also opened the comparison: the Current
  image has the vertical crack and white flake, and the AI proposal removes
  them. The report keeps the visible MOCK ONLY rationale; it is not a Human
  Decision claim. Screenshot SHA-256:
  2533872c86e3737ba81b8e23122e824ed71c8fb2535774bce83d88eba257a9be.
  Report SHA-256:
  92f4cd7c6a20a8bb22274baf58710f86914115974491a1cb4de1db44cb2d9079.
- The preliminary review-before screenshot briefly displayed loading text even
  though its images had decoded. The final report screenshot has all images
  loaded; record this as capture timing, not a persistent product issue.
  S1/S2/S3 passed for this mock flow. Reviewer disposition: accepted.

M1–M4 used a task-local predecessor of the published helper. Its original
file-byte hashes were independently checked against every public PNG and
report. The published generic helper separates file-byte and report-text
hashing, enforces the MOCK ONLY rationale marker, validates pending exit 1 and
clean fsck counts, and checks matching scenario state. Its corrected flow was
also exercised separately as English mobile MOCK Reject smoke trials (R2 and
R3). R3 used the final marker, exit-status, and fsck-count assertions; its PNG
SHA-256 was independently checked against the actual file.

## Reproduce a synthetic flow

Run from the repository root with release `synapse` / `synapse-local` binaries
and the existing `scripts/browser` Playwright dependencies and Chromium
installed. A fresh build uses `cargo build --release -p synapse-cli -p
synapse-local-http --locked`; browser setup uses `npm ci --prefix
scripts/browser --ignore-scripts` and
`scripts/browser/node_modules/.bin/playwright install chromium`.

```bash
mock_trial_root="$(mktemp -d)"
node scripts/browser/run-mock-evaluation.mjs prepare \
  --root "$mock_trial_root" --binary-dir target/release \
  --scenario T1 --provider codex --locale ja --viewport desktop
```

Open `T1/screenshots/review-before.png`, `review.png`, and `comparison.png`
inside that scratch directory before choosing an explicit synthetic test
value. The example below records MOCK Adopt, never a person's decision.

```bash
node scripts/browser/run-mock-evaluation.mjs complete \
  --root "$mock_trial_root" --binary-dir target/release \
  --scenario T1 --provider codex --locale ja --viewport desktop \
  --decision adopt --rationale "MOCK ONLY: local synthetic test; no human participant."
```

Then visually inspect `T1/screenshots/report.png` and inspect
`T1/public-evidence.json`. The commands replay the CLI/browser flow; they do
not reproduce provider model routing or constitute real-user evaluation.

## Release-gate review

Codex accepted the measured results and visually inspected all four cases.
Gemini reviewed the exact #192 completion draft and accepted it after adding
reproduction commands; this review was text-only. Claude Opus 5.5 completed a
focused text review after the broader review timed out. It supported Go for
this scope and requested consistent final review flags, accurate quotation,
and helper provenance; those corrections are reflected here and in #192.
The generic helper also enforces the mock rationale marker. No product S1,
S2, or S3 was observed in the tested flows.

The team accepts release condition 2 under the user's explicit AI mock
replacement. #192 closes when this record merges in PR #197. Final head/main
CI and the two-platform release workflow remain required before publication.
These findings establish no human UX, authentic Human Decisions, native
macOS behavior, or server-restart coverage.
