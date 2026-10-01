# Participant tasks (Creator pilot v2)

You use a SynapseGit release candidate (a version made before the official
v1.0 release) together with your AI agent. You ask the agent to run the
commands: setup, writing a candidate, reading back, and backup.
**You record Adopt, Reject, or Defer yourself on the browser page. Do not let
the agent decide.**

If something is unclear, try to continue on your own first. When you are
stuck, ask the facilitator. Where you needed help is also a useful record.

## Before you start

The facilitator gives you:

- the version to use (for example `v1.0.0-rc.1`) and how to install it
- where the three practice images are (if you chose to use your own work in
  the consent sheet, prepare it yourself)

Install by following the [installation guide](../../../install.md). You may
ask your AI agent to do it. If you do, do not let it skip the checksum and
build provenance checks.

The release archive contains a guide for AI agents (`AI_AGENT_GUIDE.md`; the
Japanese one is `AI_AGENT_GUIDE.ja.md`). Ask your agent to read it first.

### The three images

| Role | Meaning |
|---|---|
| Original | The state before any change |
| Current | The present state (what was given to the AI, or what you will work on) |
| AI output | The candidate an AI made. **SynapseGit did not make it.** The file you made with another tool or AI is recorded as it is |

## Task 1: Ask for a practice project

Ask your AI agent, for example (any wording is fine):

> We are using SynapseGit. First read the bundled AI_AGENT_GUIDE.md.
> For practice, set up a repository at `$HOME/SynapseGit-pilot/work` and an
> inbox at `$HOME/SynapseGit-pilot/inbox`.

The agent runs `synapse init` and similar commands. Read its explanation of
what it did.

## Task 2: Ask it to place a candidate in the inbox

> Put the three practice images in the inbox as Original, Current, and AI
> output. Name the candidate `mural-pilot-1`, with the subject "North wall
> mural" and my display name as the creator (it need not be my real name).
> Add a generation note with what is known, such as the tool that made the
> AI output.

The agent uses `synapse inbox put`. Nothing is recorded yet: no proposal or
decision exists until you review the candidate on the page.

## Task 3: Ask it to open the page

> Start synapse-local and tell me the URL. I will decide, so do not choose
> for me.

Open the `http://127.0.0.1:...` URL in your browser. This page runs only on
your computer.

## Task 4: Review the candidate and record a decision

1. Open the project. The Sessions page says that candidates are waiting to be
   imported.
2. Open the Import page and select Review for `mural-pilot-1`.
3. Check the three images and the names and generation note that the agent
   added. Correct the names if needed, then create the proposal.
4. On the review page that opens:
   - Use "Compare images at full size" to compare Current and AI output.
   - Read what the "File-content match check" says.
5. Write a rationale (optional) and choose Adopt, Reject, or Defer. When a
   confirmation dialog appears, read it before you decide. Any choice is
   fine; adopting the AI's candidate is not the right answer.

## Task 5: Read the record back

1. On the page shown after recording, read the recorded decision and
   rationale.
2. Ask your AI agent to show the same record with commands:

   > Show me the list of recorded sessions and the report for the session I
   > just decided.

   The page suggests the candidate name with `inbox-` in front as the session
   name (for example `inbox-mural-pilot-1`). If you changed the name, tell the
   agent.

3. Check that the page and the agent's explanation show the same decision.

## Task 6: Make a backup

> Stop synapse-local, then make a backup of the practice project at
> `$HOME/SynapseGit-pilot/backup-1`.

The agent uses `synapse export`. The backup stays on your computer. The
facilitator may check with you that the backup can be restored into an empty
project.

## Task 7 (optional): A second candidate

If there is time, repeat Tasks 2 to 5 with another candidate. For example,
choose a different decision (such as Defer), or use "Try a next candidate
from this record" on the page of a completed record.

## Finally

Answer the questions in [`questions-en.md`](./questions-en.md) in your own
words. You may look back at the pages and documents while you answer.

## If something goes wrong

- If the agent tries to press a decision button on the page, or to record a
  decision with something like `creator-run --decision`, stop it and tell the
  facilitator. Only a person records a decision.
- If the page shows `incomplete` or an unfamiliar error, tell the facilitator
  as it is. Do not delete records or move files yourself.
- If you are using your own work and feel uneasy, you can stop at any time.
