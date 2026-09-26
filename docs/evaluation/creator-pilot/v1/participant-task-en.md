# Participant task sheet (English)

This sheet walks you through recording your first creative decision in
SynapseGit yourself. It is not a test with a pass/fail score. Please tell the
facilitator about anything confusing, anything you needed help with, and
anything that felt cumbersome.

## 0. Before you start

- **Target version**: v0.9.0.
- **Three binaries are involved**:
  - `synapse` — the command-line tool used to record and inspect a decision.
  - `synapse-local` — the loopback-only browser application.
  - `synapse-present` — turns a recorded session into a read-only bundle for
    sharing outside SynapseGit.
  - The facilitator will normally have these installed already. To install
    them yourself, follow the [installation guide](../../../install.md).
- **Prerequisites**:
  - A terminal you can run `synapse` / `synapse-local` in.
  - A browser that can open a loopback address on your own machine
    (`http://127.0.0.1:...`).
  - **The application's browser screens are Japanese-only in this version.**
    If you do not read Japanese, expect to need facilitator help reading the
    UI. An English UI is not part of this release.
- **Roles of the three images used in this exercise** (synthetic practice
  images, not a real mural):
  1. **Original** — an earlier reference state before visible damage.
  2. **Current** — the present observed state, with a crack, flaking, and
     discoloration.
  3. **AI output (AI-attributed proposal)** — a third image offered as a
     treatment proposal. **SynapseGit does not generate this image with a
     model.** Someone (in this exercise, the facilitator, ahead of time)
     supplied the file, and SynapseGit records it as "the AI-attributed
     candidate" with that limited meaning only.
- **If you later prepare your own images** (this exercise itself uses the
  practice images; use this as guidance for later):
  - Use three images you have the rights and permission to use.
  - Original and Current should be the same subject at two different points
    in time; AI output is the third candidate image you want to compare.
  - SynapseGit compares and records the exact **bytes** of each file, not
    its visual appearance. A visually edited image with different bytes is
    recorded as a different file even if it looks unchanged.
  - Keep each file at or under 64 MiB, and the three files at or under
    192 MiB combined, when using the browser (localhost) path.

## 1. Import the three images and compare the AI output

The facilitator will tell you whether to do this task from the **CLI
(terminal)** or the **browser UI (localhost)**. Both record the same kind of
content in the end.

### 1-A. CLI path

Confirm the practice repository path and session name with the facilitator,
then run the following with REPO, SESSION, ORIGINAL, CURRENT, AI_OUTPUT,
SUBJECT, CREATOR, DECISION, and RATIONALE replaced with the actual values:

```bash
synapse creator-run REPO SESSION \
  ORIGINAL CURRENT AI_OUTPUT \
  --subject "SUBJECT" \
  --creator "CREATOR" \
  --decision DECISION \
  --rationale "RATIONALE"
```

- Use the decision you chose in Task 2 for DECISION (`adopt`, `reject`, or
  `defer`).
- Review the command's output right after it runs.

### 1-B. Browser UI (localhost) path

Open the project page the facilitator started with `synapse-local` (use the
`http://127.0.0.1:...` URL the facilitator gives you).

1. Start a new session and fill in Session (session name), Creator name
   (display name), and Subject label (the name of the subject).
2. Choose the Original, Current, and AI output (AI-attributed proposal)
   images in their respective fields. Selecting files alone does not submit
   anything.
3. Review the preview, then create the Proposal.
4. On the session page, open "画像を拡大して比較" (enlarge and compare) and
   choose the two images you want to compare (for example, Current and AI
   output). "重ねて表示" (overlay) is only selectable when the two chosen
   images have matching decoded width and height; when it is available, you
   can adjust the opacity to see them overlaid from a shared top-left origin
   (it does not align images or auto-detect differences). Otherwise only
   "並べて表示" (side by side) is available.
5. Continue into Task 2 on the same page: enter your decision (Adopt/Reject/
   Defer) and rationale, then click one of the decision buttons. **The
   browser will show a native confirmation popup (OK/Cancel).** Read it and
   choose OK to record the decision, or Cancel to record nothing (your
   rationale text is kept in the field either way).

## 2. Record an Adopt, Reject, or Defer decision with a rationale

Before running Task 1, read the description of each choice below and decide
which one you will use.

- **Adopt** — select the supplied AI output unchanged.
- **Reject** — do not select the AI output; the Current state is retained.
- **Defer** — postpone the adoption decision; the Current state is retained,
  and the record shows that the decision was deferred.

Once decided, write a short rationale (via `--rationale`, or the rationale
field in the browser). Free-form text is fine.

**This decision cannot be changed or reopened once confirmed, in this
session.** Choosing Defer does not create a resumable draft — it completes
that session's decision as "deferred."

## 3. Read the recorded decision back

After recording the decision, read back what was actually saved. If you used
the CLI, run (REPO and SESSION are the same values as in Task 1-A):

```bash
synapse creator-report REPO SESSION
```

Check the output for:

- `disposition=` showing the decision you chose (adopt/reject/defer).
- `selected=` (only `true` for `adopt`).
- `byte_identity=` (`identical` or `different`).
- Your rationale, displayed exactly as recorded.

If you used the browser UI, check the same information (decision and
rationale) on the completed session page, under the recorded-decision
section. If the facilitator asks, compare that display with
`creator-report`'s output.

## 3b. Ground where public-facing text comes from, without publishing anything

This task shows where text meant for other people is prepared.
**Nothing is published or sent anywhere in this task — you only look at a
local screen or local files.**

- **Browser path**: open "公開用の制作ノートを作る" (create a public-facing
  creative note) from the project page. Confirm that you can select the
  session you completed in Task 1, then note what each field contains when
  the page opens. Do not submit the form.
- **CLI path**: using a destination path the facilitator gives
  you that does not exist yet, called `OUT` below, run:

  ```bash
  synapse-present export REPO OUT --session SESSION --public
  ```

  Open the generated files under `OUT` (such as `index.html`, `story.md`,
  and `projection.json`), look for the rationale text you entered, and
  note what you find.

Use what you observed here for comprehension question 4.

## 4. (Optional) Try the next candidate in a separate session

If there is time, follow the facilitator's instructions to repeat Tasks 1–3
in **a new session** (the same base images or a different AI output image is
fine). **A session name that has already been used cannot be reused** —
create a new session with a new name.

## After the tasks

Answer the comprehension questions (`comprehension-questions-en.md`) in your
own words. You do not need to look anything up to verify your answer. "I
don't know" is a fine answer if that is genuinely the case.
