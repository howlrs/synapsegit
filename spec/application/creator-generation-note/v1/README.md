# Creator generation note v1

This Creator application contract uses the immutable AI Activity envelope's
`extensions["org.synapsegit.creator-generation-note"]`. Core record schemas,
Actor attribution, ContextPack, caller-supplied output and Human authority retain
their existing meaning. No execution or authorship verification is implied.

The extension has exactly `format` (`synapsegit-creator-generation-note-v1`),
`attribution` (`user_declared`), `activity_id`, `ai_output_blob_oid`, and `note`.
The activity ID must equal the containing Activity entity ID, and the Blob OID
must equal that Activity's verified proposal output. The proposal Commit binds
the Activity through its transition and snapshot. Notes cannot refer to their
own enclosing Commit OID without a hash cycle. Report checks the binding to the
exact Proposal through these existing edges. Unknown fields, versions, malformed
notes and mismatched bindings are rejected by the Creator reader.

`note` contains optional plain strings `tool`, `model`, `prompt`, `intent`, with
UTF-8 byte maxima 300, 300, 8192 and 2048 respectively. Missing strings default to
empty. The serialized compact note JSON must be at most 16384 bytes, including
JSON escaping. Input is never truncated. The writer validates before any Ref
publication and reads its constructed extension back before publication. Empty
notes create no extension; existing sessions read as generation note absent.
`valid.json` is a contract example, not an independently runnable Core record.

Notes are local/private and immutable after creation. Normal Core archives
include them as private production records, and restore preserves the binding.
Pending/complete UI and creator-report display them separately from rationale.
Public projection and human/machine publication views do not copy them or use
these strings as model identity. Error text never interpolates note contents.

The localhost multipart input accepts four optional text/plain UTF-8 fields:
`generation_tool`, `generation_model`, `generation_prompt`, `generation_intent`.
The service DTO has optional `generation_note`; the trusted Creator API adds
`begin_creator_session_with_note` while preserving the existing begin/run inputs.
No model invocation, note editing, structured seed/JSON parameters or cloud sync
is provided.
