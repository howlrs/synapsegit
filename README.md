# SynapseGit

SynapseGit is a Git-like Core for preserving creative intent, evidence,
observations, decisions, and future reinterpretation without treating a digital
record as physical truth.

Status: **Core v0.1 / Stage 0 draft**

## Start here

### User guide

- [Intended-user scenarios / 想定利用者別シナリオ（PPTX・日本語・branch: main）](https://github.com/howlrs/synapsegit/blob/main/docs/presentations/synapsegit_user_scenarios_ja.pptx)
- [Usage guide / Stage 0の使用方法（日本語・branch: main）](https://github.com/howlrs/synapsegit/blob/main/docs/usage_guide.md)
- [Presentation usage and regeneration / PPTX利用・再生成手順（branch: main）](https://github.com/howlrs/synapsegit/blob/main/docs/presentations/README.md)

### Design and protocol

- [Core concept](docs/core_concept.md)
- [Stage 0 execution plan](docs/stage0_execution_plan.md)
- [Runtime architecture](docs/runtime_architecture.md)
- [Core Protocol v0.1](spec/core/v0.1/README.md)
- [OID profile](spec/core/v0.1/oid-profile.md)
- [Operations and semantic validation](spec/core/v0.1/operations.md)

The current runtime direction is Rust for canonicalization, OIDs, CAS, closure,
and archive verification; a filesystem/object-storage CAS as the source of
truth; SQLite for the initial local RefStore and projection; and SurrealDB as an
optional graph projection that must be rebuildable from Core objects.

## Verify the Stage 0 fixtures

```bash
node scripts/verify_core_fixtures.mjs
cargo test --workspace --locked
```

The JavaScript verifier checks 20 schemas, 17 structured golden fixtures, strict
JSON and Unicode behavior, resource limits, set and parent ordering,
fixed-point/time rules, closure states, Tombstones, and empty-store restore.

The Rust workspace now implements the Stage 0 local vertical slice:

- `synapse-canonical`: resource-bounded strict JSON, canonical bytes, and OIDs;
- `synapse-schema`: offline Draft 2020-12 record dispatch plus Synapse semantic annotations;
- `synapse-cas`: atomic filesystem CAS, typed closure, Tombstone availability, and fsck;
- `synapse-sqlite`: transactional Ref compare-and-swap and reflog;
- `synapse-core`: validated ingestion and checksum-bound export/empty-store restore;
- `synapse-cli`: `put`, `update-ref`, `fsck`, `export`, and `restore` commands.

The Rust tests match all 17 structured golden fixtures and the raw Blob fixture
without sharing parser or canonicalization code with the JavaScript verifier.
Production ingestion goes through `synapse-schema`; low-level structured CAS and
OID APIs remain explicitly named `*_unchecked`.

The `sg-oid-v1` values are draft fixtures until a second independent production
implementation also completes schema and semantic validation for the Stage 0
inter-language freeze gate.

## Local CLI

```bash
cargo run -p synapse-cli -- init .synapse
cargo run -p synapse-cli -- put-blob .synapse path/to/file
cargo run -p synapse-cli -- put-record .synapse path/to/record.json
cargo run -p synapse-cli -- build-tree .synapse path/to/tree.json
cargo run -p synapse-cli -- commit .synapse path/to/commit.json
cargo run -p synapse-cli -- update-ref .synapse proposal/agent/run-1 - <commit-oid>
cargo run -p synapse-cli -- fsck .synapse
cargo run -p synapse-cli -- export .synapse archive.sg
cargo run -p synapse-cli -- restore archive.sg restored.synapse
```

`-` means that the Ref must not yet exist. Later updates must supply the exact
current Commit OID as `expected_head`; stale updates fail without changing the
Ref or reflog.
