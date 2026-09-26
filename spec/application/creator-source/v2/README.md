# Creator three-Blob reuse source v1

`synapsegit-creator-reuse-source-v1` is a private extension on the new
session's import Activity (`org.synapsegit.creator-reuse-source`).  It binds a
new Human review to exact Original, Current, and AI-output Blob OIDs plus the
fixed source Proposal and Decision heads. `kind` is either
`interrupted_pending` or `deferred_rereview`.

This is provenance evidence only. It does not restore a prior Human permit,
copy a rationale, generation note, or pins, or claim that the AI output was
executed again. The source heads are typed input edges and are rechecked with
the new Ref publication; a stale source fails without changing either source
Ref. A pending review retained by the running process is never eligible.

The existing `synapsegit-creator-source-v1` remains unchanged for the
two-Blob/new-candidate derivation workflow. Readers reject unknown extensions;
therefore v0.8.x and earlier binaries cannot read a repository that contains
this form. Archive export/restore retains it, while frozen public profile v1
and its sidecar form reject every session with either source binding.
