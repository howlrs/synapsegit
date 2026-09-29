# Creator workflow

[日本語](./creator_workflow.md)

This focused guide is for the v0.12.0 release binary. It explains the path
after the [15-minute mural tutorial](./tutorial/README.md): record your own
three images, keep optional private notes, make a Human Decision, and begin a
fresh review when another candidate is needed.

Install the release from the [installation guide](./install.md), then use the
[English README startup command](../README.md#3-inspect-it-locally) to start
`synapse-local`. The [local application runbook (Japanese)](../deploy/local/README.md)
is an optional detailed reference. The browser interface is single-user and
serves only IPv4 loopback. The security limits are summarized in the
[privacy and trust summary](./security_model.en.md).

## Record a proposal and optional generation notes

On a project page, create a new session with a session name, display name,
and exactly three files: Original, Current, and an AI-attributed output. The
output is a file you supply. SynapseGit does not call a model or verify that a
model generated it. Selecting a file does not upload it.

You may add user-declared tool, model, prompt, and creative intent before
creating the Proposal. These notes are optional and immutable after saving;
they do not prove model execution or authorship. Limits are UTF-8 bytes:

| Field | Limit |
|---|---:|
| Tool | 300 |
| Model | 300 |
| Prompt | 8,192 |
| Creative intent | 2,048 |
| Serialized generation note | 16 KiB |
| One image / all three images | 64 MiB / 192 MiB |

The project overview lists at most 200 unverified summaries, with pending
reviews first and then recent Ref updates. Open a session page or run `fsck`
to verify a record; notes and rationales are not shown in the overview.

The CLI can attach the same private note with a UTF-8 JSON file. For example,
save `{"tool":"image editor","model":"model-a","prompt":"Restore the blue area","intent":"Compare a restrained option"}`
as `generation-note.json`, then run:

```sh
synapse creator-run /path/to/repository session-1 original.png current.png candidate.png \
  --subject "My work" --creator "My display name" --decision defer \
  --generation-note-file generation-note.json
```

The file is parsed and validated before the repository is opened. `--decision`
is still required in this invocation; the CLI does not resume a Human Decision
later. Protect the note file, repository, and ordinary archive as private data.

## Pin an image and make the Human Decision

During a pending review, choose a displayable image and add up to ten private
decision pins. You can position a pin by pointer, keyboard, or integer
coordinates. Each pin permits up to 200 UTF-8 bytes. The overall JSON request,
including pins and rationale, is limited to 8,192 bytes.

Add an optional rationale, read the displayed outcome, and explicitly confirm
one decision:

- `adopt` selects the candidate;
- `reject` keeps the base state; and
- `defer` completes the decision while postponing selection.

Pins and rationale are saved with that one Human Decision. There is no
per-pin adoption, partial adoption, or editing/reopening of the same session.
Pending review authority is process-local: after a restart, do not recreate it
from values displayed in the browser.

The completed page shows the notes, rationale, and pins. Its **Save private
record (JSON)** control fetches the existing authenticated session-detail endpoint again
and downloads the fresh native complete response unchanged:
`{"state":"complete","report":{...}}`. It is shown only for a complete
session; it is absent while a review is pending or incomplete, and it produces
no file if the fresh verification fails. The response can contain private
rationale, `generation_note`, pins or annotations, internal identifiers, and
source lineage when present. Keep it private. It is neither a public bundle nor
a repository backup, and it is not interchangeable with the CLI JSON document.

The CLI report also
shows generation notes as `generation_note_user_declared` and valid pins as
`decision_pins_private`:

```sh
synapse creator-report /path/to/repository session-1
```

The built-in image comparison is manual viewing only. It can show two images
side by side or overlay equal decoded dimensions, but performs no registration,
pixel difference analysis, EXIF inspection, or visual similarity judgment.

## Try another candidate or review one again

From a completed Adopt, Reject, or Defer session, choose the option to try
another candidate. SynapseGit reuses the verified exact Original and Current
bytes in the same project, takes one new candidate image, and starts a fresh
session with a new identity, Refs, and Human review. It does not copy the old
generation notes or rationale. An adopted AI output does not become Current,
and reuse does not establish a new observation or prove a shared physical
subject.

An interrupted proposal that can be verified, or a completed Defer, can also
start a separate fresh review using its recorded three images. The old
decision stays fixed. Source lineage is private and survives ordinary archive
export and restore. The frozen public v1 format refuses a reused or re-review
session, including a whole-project export containing one; select a complete,
non-derived session with `--session` for a compatible existing v1 export.

For a Defer re-review, the confirmation and new-review page show the original
generation note, Defer rationale, and decision pins as reference only. They
are not copied into the new decision. In both interrupted and Defer flows, the
service refuses to create the new session if the confirmed source head has
changed. If it cannot read the fixed source record, the UI reports that fact.

## Bring in a script-output candidate through Inbox

Start the local service with `--import-root PROJECT=INBOX` only when you have
an appropriate local producer. The producer writes three images to
`INBOX/<slug>` and writes the strict manifest last. The browser sends only the
logical slug, previews the manifest-last candidate, and requires confirmation
before it begins a Proposal from retained bytes. Closing that confirmation
discards the process-private staging. Inbox does not make the producer or its
output trusted, and it does not invoke a model.

## Prepare public text separately

For one complete, non-derived session created with the normal three-file
import, the project page can create a public-text `presentation.toml` from
empty author-supplied fields. It does not copy private notes, pins, prompts,
or rationales. Downloading the file makes no Core or Ref write and does not
create, upload, or share a bundle. Follow the [public-text workflow](./presentation_sidecar.en.md)
for limits and the separate bundle command.

## What stays private

Ordinary Core archives retain generation notes, decision pins, and fixed
private source lineage along with the repository history. A public bundle has
a different boundary: it excludes those private notes and raw assets, but it
can still contain identifiers and author-supplied public text. Read the
[privacy and trust summary](./security_model.en.md) before distributing an
archive or copying a bundle outside your control.

For contract details, use the [generation-note specification](../spec/application/creator-generation-note/v1/README.md),
[decision-pin specification](../spec/application/creator-decision-pins/v1/README.md),
[source-lineage specification](../spec/application/creator-source/v1/README.md),
and [reuse-source specification](../spec/application/creator-reuse-source/v1/README.md).
The [CLI reference (Japanese)](./cli_reference.md) is the command and error
reference.
