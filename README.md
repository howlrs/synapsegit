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

The independent Rust implementation in `crates/synapse-canonical` implements
the resource-bounded canonicalization and digest layer without sharing parser
or canonicalization code with the JavaScript verifier. Its tests match all 17
structured fixtures and the raw Blob fixture on canonical length, canonical
SHA-256, and `sg-oid-v1`. Structured OID entry points remain explicitly
unchecked until the schema and semantic validation crate supplies the validated
ingestion path.

The `sg-oid-v1` values are draft fixtures until a second independent production
implementation also completes schema and semantic validation for the Stage 0
inter-language freeze gate.
