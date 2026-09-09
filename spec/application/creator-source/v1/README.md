# Creator reused source v1

The private import Activity extension `org.synapsegit.creator-source` records
`format: synapsegit-creator-source-v1`, source session, fixed Proposal and Decision
Commit OIDs, disposition (`adopt`, `reject`, `defer`), and Original/Current Blob OIDs.
All fields are required; unknown fields and versions are rejected. The import's
canonical `input_refs` contain exactly the typed `source_decision` and
`source_proposal` edges, retaining both immutable closures through Core archives.
No Core schema or ownership/identity semantics change.

Before publication, the source must be a complete verified session in the same
repository, both reused input byte hashes must match, and source Ref heads must
still match within the initial publication transaction. Missing, corrupt or
tombstoned closure objects fail validation. Subsequent reports validate the fixed
heads using the derived session's reachable graph, independently of later source
Ref movement. Chains are bounded to 16 derivations.

The localhost service resolves an opaque confirmation within the registered
project and current server process, with at most 16 confirmations per project and
64 total. It revalidates source previews and creation against the captured binding,
uses private server-owned staging with 64 MiB per image, and serializes writes.
The browser submits only that confirmation, new display names/session, one fresh
candidate, and optional fresh generation notes. It cannot select source OIDs or paths.

Reused Current is the source's recorded Current, even after Adopt. Reuse is not a
new capture, verified physical condition, or proof of a shared real-world identity.
The new session has fresh local identities and one-shot Human review. Source notes
and decisions are not copied as new candidate notes. Ordinary archives retain this
private lineage; public projection does not automatically expose it.
