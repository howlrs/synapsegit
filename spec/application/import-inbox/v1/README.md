# Import inbox manifest v1

An inbox producer writes `original`, `current`, and `ai-output` in a directory
named by an ASCII slug, then writes `manifest.json` last. The manifest uses
`synapsegit-import-inbox-v1` and is validated against
[`manifest.schema.json`](./manifest.schema.json).

Each file entry names one leaf file, declares its byte size (at most 64 MiB),
and can include a SHA-256 digest. `metadata.generation_note`, when present,
contains all four fields from the Creator generation-note contract; empty
strings are valid values. The inbox is caller-supplied input, not proof of a
model invocation or authorship.

The localhost service receives only a configured project key and logical slug.
It opens the configured root, candidate, and leaves without following links,
copies verified bytes into private process staging, and creates a proposal only
from those staged bytes. Removing a preview in the browser discards its stage.

A file `name` is one leaf without NUL, `/`, or `\`, as the schema requires;
the service rejects any other name. `synapse inbox put` is the reference
producer. It writes the fixed names `original`, `current`, and `ai-output`,
always records `sha256`, and builds the candidate in a hidden directory whose
name is not a slug. It then renames that directory to the slug without
replacing an existing entry, so the manifest is present when the candidate
first becomes visible.
