# SynapseGit

SynapseGit is a Git-like Core for preserving creative intent, evidence,
observations, decisions, and future reinterpretation without treating a digital
record as physical truth.

Status: **Core v0.1 / Stage 0 draft**

## Start here

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
```

The verifier checks 20 schemas, 17 structured golden fixtures, strict JSON and
Unicode behavior, set and parent ordering, fixed-point/time rules, closure
states, Tombstones, and empty-store restore.

The `sg-oid-v1` values are draft fixtures until a second independent production
implementation completes the Stage 0 inter-language freeze gate.
