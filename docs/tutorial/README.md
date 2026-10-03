# SynapseGit 15-minute mural tutorial

[日本語](./README.ja.md) | [Back to the main README](../../README.md)

This tutorial records one synthetic conservation decision from beginning to
end. You will keep three image states, attribute one state as an AI proposal,
make a Human Decision, inspect the result in the localhost application, and
produce a read-only presentation bundle.

Running this tutorial does not call a model, cloud service, or GitHub API.
The three images are synthetic fixtures, refreshed with the built-in `image_gen`
tool on 2026-10-02. [Prompts and image hashes](../assets/image-generation.json)
record this revision; no backend model ID was exposed by the tool.
Published v1.0.0-rc.1 archives retain the earlier images. No real artwork or
conservation treatment is documented here.

## What you will create

[![Prepare a candidate, compare three images and make a human decision, then revisit the record](../assets/creator-journey.en.png)](../assets/creator-journey.en.png)

_Concept illustration of the walkthrough. The three files below are the actual
inputs; the later application images are real screen captures._

| Original reference | Current observation | AI-attributed proposal |
|---|---|---|
| ![Synthetic coastal mural before visible damage](./assets/mural-original.png) | ![Synthetic coastal mural with a crack and a missing-pigment patch](./assets/mural-current.png) | ![Synthetic restrained conservation proposal](./assets/mural-ai-proposal.png) |
| Earlier reference state | A crack and paint loss are visible | A restrained digital treatment proposal |

SynapseGit stores these as opaque bytes. The descriptions above are tutorial
context written by us; the current byte-identity adapter does not derive those
visual interpretations.

## Before you start

Install `synapse`, `synapse-local`, and `synapse-present` by following the
[installation guide](../install.md). Confirm:

```bash
synapse --version
synapse-local --version
synapse-present --version
```

This walkthrough uses a new repository. Pick an empty path and stop any
`synapse-local` process that already owns it.

```bash
export SYNAPSE_TUTORIAL_REPO="$HOME/SynapseGit/mural-tutorial"
test ! -e "$SYNAPSE_TUTORIAL_REPO"
```

If that path already exists, choose another path. The tutorial never deletes or
replaces an existing repository.

## 1. Record the proposal and Human Decision

Run this command from the cloned SynapseGit repository, or from the extracted
v1.0.1 release archive (it bundles the same
`docs/tutorial/assets/` paths; see `TUTORIAL.md` at the archive root), so the
sample paths resolve:

```bash
synapse init "$SYNAPSE_TUTORIAL_REPO"
synapse creator-run "$SYNAPSE_TUTORIAL_REPO" mural-treatment-01 \
  docs/tutorial/assets/mural-original.png \
  docs/tutorial/assets/mural-current.png \
  docs/tutorial/assets/mural-ai-proposal.png \
  --subject "Community Hall Coastal Mural" \
  --creator "Tutorial Conservator" \
  --decision adopt \
  --rationale "Adopt the restrained inpainting proposal as the next documented state."
```

The output prints immutable Blob IDs and the Proposal and Decision Ref heads.
Your IDs and timestamps will differ from this documentation because a new
session creates fresh actors and records.

What just happened:

1. the three exact files became content-addressed Blob objects;
2. SynapseGit recorded the original and current observations;
3. the third file became a caller-supplied, AI-attributed proposal;
4. `adopt` recorded a Human Decision selecting the proposal unchanged; and
5. repository integrity was checked before the command completed.

`AI-attributed` does not mean SynapseGit generated or authenticated the image.
The caller supplied it, and the workflow records that limited attribution.

## 2. Read the decision report

```bash
synapse creator-report "$SYNAPSE_TUTORIAL_REPO" mural-treatment-01
```

Look for:

```text
disposition=adopt
selected=true
ai_output_source=caller_supplied
comparison_comparability=partial
byte_identity=different
comparison_warning="Different Blob bytes do not establish visual or physical change."
fsck=clean
```

This combination is intentional. SynapseGit can verify which bytes were stored
and selected. It does not infer that the crack is real, that the treatment is
good, that a model produced the proposal, or that anyone owns the rights.

## 3. Inspect the actual localhost UI

Start the local application:

```bash
synapse-local \
  --project "mural=$SYNAPSE_TUTORIAL_REPO" \
  --label "mural=Community Hall Coastal Mural"
```

Open the exact `http://127.0.0.1:...` origin printed in the terminal. Do not
expose it through a reverse proxy.

Use the **English** or **日本語** control in the page header to choose the
display language. An explicit choice is stored in this browser and remains in
effect when you move between pages or reload. Without one, the application
uses a supported browser language preference and otherwise starts in Japanese.
Only application labels and messages change: your subject, notes, rationale,
stored history, API identifiers, and error codes remain exactly as recorded.
The tagged v0.13.1 binary includes this language selector. A header choice is
retained in the browser and takes priority over `Accept-Language`.

![Actual SynapseGit Local overview generated from this tutorial repository](./assets/tutorial-overview.png)

_Actual `synapse-local` v1.0.0-rc.1 output with the 2026-10-02 synthetic images.
It shows one completed session and no pending reviews.
[Capture provenance](../assets/synapse-local/capture.json) records the runtime and image hashes._

Open the project and the completed session to inspect:

- Original, Current, and AI output images;
- the Human `adopt` Decision and rationale;
- Proposal and Decision Refs;
- the comparison limitation and replay readiness; and
- the four-event timeline.

On that completed session, choose **Save private record (JSON)** when you need a
local copy for permitted private review. The browser refetches and verifies the
existing authenticated session detail at click time, then saves the native
`{"state":"complete","report":{...}}` response. It may contain the private
rationale, `generation_note`, annotations or pins, internal identifiers, and
source lineage. Do not use it as a public presentation bundle, a repository
backup, or a replacement for `synapse creator-report --format json`. Pending
and incomplete sessions do not show the control; a failed fresh read does not
produce a file.

To make a new decision in the browser, open a project's **Import** page, use
**Start from three files** to add Original, Current, and AI output, then select
**Create proposal**. On the review page, **Adopt**, **Reject**, and **Defer** each
finish that session once recorded. A Defer is not an open review; use the
separate new-session review action if you want to consider the recorded images
again.

For a detailed completed-session view, see this additional capture from the
same implemented UI:

![Actual SynapseGit Local completed creator session](../assets/synapse-local/creator-session.png)

Press Ctrl-C in the terminal before using the CLI against the same repository
again.

## 4. Export and verify a local presentation

Create a human- and machine-readable bundle:

```bash
synapse-present export "$SYNAPSE_TUTORIAL_REPO" \
  "$HOME/SynapseGit/mural-tutorial-public" \
  --session mural-treatment-01 \
  --presentation docs/tutorial/presentation.toml \
  --github
```

Preview and verify it:

```bash
synapse-present preview "$HOME/SynapseGit/mural-tutorial-public"
```

`preview` verifies the fixed inventory, checksums, schemas, canonical
projection, manifest links, and target copy. The bundle contains canonical
JSON, Markdown, script-free HTML, a manifest, checksums, and a local target
layout. Neither command contacts or publishes to GitHub. Review every generated
byte before sharing it.

## 5. Try a different Human Decision

Create a fresh repository path and repeat step 1 with:

- `--decision reject` to retain the base state and reject the proposal; or
- `--decision defer` to retain the base state and postpone selection.

Sessions are create-only. Do not reuse `mural-treatment-01` in the same
repository.

## What this tutorial demonstrates

| Demonstrated | Not demonstrated |
|---|---|
| Exact input byte identity | Pixel registration or visual similarity |
| AI-attributed proposal separated from Human Decision | Verified model execution |
| Immutable objects and mutable Ref heads | Authorship, truth, rights, or permission |
| Human selection of `adopt`, `reject`, or `defer` | Physical conservation work |
| Local report, UI, integrity check, and presentation | Hosted collaboration or remote publication |

## Troubleshooting

- **`synapse: command not found`** — revisit the [installation guide](../install.md)
  and confirm that the binary directory is on `PATH`.
- **Repository or session already exists** — choose a new repository path or
  session slug. The Pilot does not overwrite a prior creative history.
- **The browser cannot connect** — keep the terminal process running and open
  the exact loopback URL it prints.
- **A session becomes incomplete after restart** — pending Human review
  authority is same-process in the tagged v0.8.0 binary. Diagnose it from the UI; do not reconstruct
  authority from displayed identifiers.
- **The images look visually different but the report says only
  `byte_identity=different`** — that is the current conservative boundary.

To continue with your own images, follow the [Creator workflow](../creator_workflow.en.md)
and then the [public-text workflow](../presentation_sidecar.en.md). For command
details, see the [CLI reference (Japanese)](../cli_reference.md) and [usage
guide (Japanese)](../usage_guide.md). Before using your own data or sharing a
bundle, read the English [privacy and trust summary](../security_model.en.md).

## Next step

To let an AI agent run the CLI while you make the decisions, give it the
[AI agent guide](../ai_agent_guide.md).

If you want to check whether SynapseGit fits your own creative work, try the
[Creator pilot evaluation kit](../evaluation/creator-pilot/v1/) (Japanese,
with an English summary). It reuses this same tutorial to walk through
comprehension questions and observation notes for your own first recorded
decision. Two of its files are already in English:
[participant task sheet](../evaluation/creator-pilot/v1/participant-task-en.md)
and
[comprehension questions](../evaluation/creator-pilot/v1/comprehension-questions-en.md).
For a real-work follow-up, use the [first real-work Pilot start checklist
(Japanese)](../creator-pilot/start-checklist.ja.md).
