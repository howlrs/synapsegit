# SynapseGit 15-minute mural tutorial

[日本語](./README.ja.md) | [Back to the main README](../../README.md)

This tutorial records one synthetic conservation decision from beginning to
end. You will keep three image states, attribute one state as an AI proposal,
make a Human Decision, inspect the result in the localhost application, and
produce a read-only presentation bundle.

No model, cloud service, GitHub API, or real artwork is involved. The three
images are synthetic tutorial fixtures generated for this repository.

## What you will create

| Original reference | Current observation | AI-attributed proposal |
|---|---|---|
| ![Synthetic coastal mural before visible damage](./assets/mural-original.png) | ![Synthetic coastal mural with a crack, paint loss, and water discoloration](./assets/mural-current.png) | ![Synthetic restrained conservation proposal](./assets/mural-ai-proposal.png) |
| Earlier reference state | Crack, flaking, and discoloration are visible | A restrained digital treatment proposal |

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

Run this command from the cloned SynapseGit repository so the sample paths
resolve:

```bash
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

![Actual SynapseGit Local overview generated from this tutorial repository](./assets/tutorial-overview.png)

_Actual `synapse-local` output from the tutorial fixture. It shows two Refs,
one completed session, and no pending reviews._

Open the project and the completed session to inspect:

- Original, Current, and AI output images;
- the Human `adopt` Decision and rationale;
- Proposal and Decision Refs;
- the comparison limitation and replay readiness; and
- the four-event timeline.

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
  authority is same-process in the tagged v0.5.0 binary. Diagnose it from the UI; do not reconstruct
  authority from displayed identifiers.
- **The images look visually different but the report says only
  `byte_identity=different`** — that is the current conservative boundary.

For command details, continue with the [CLI reference](../cli_reference.md),
[usage guide](../usage_guide.md), and [security model](../security_model.md).
