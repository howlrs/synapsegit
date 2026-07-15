# SynapseGit project status

Audience: preview evaluators、contributors、maintainers
Status: public project snapshot
Applies to: v0.1.0 / main
Last verified: 2026-07-15

SynapseGit Coreは**Stage 0 draft**である。v0.1.0は、local repositoryとbounded creator
Pilotを評価するための最初のprereleaseであり、production-readyなcreator applicationや
multi-user serviceではない。

## 現在の成果

- strict JSON、canonical bytes、domain-separated OID
- concrete Record schemaとlocal semantic validation
- filesystem content-addressed ObjectStore、typed closure、Tombstone、`fsck`
- SQLite Ref compare-and-swapとreflog
- checksum-bound directory exportとverified restore
- original／current／caller-supplied AI outputを取り込む3-file creator Pilot
- AI-attributed proposalと`adopt`／`reject`／`defer`のHuman Decision
- primary Blob OIDだけを比較する保守的なbyte-identity Analysis
- timeline、decision、evidence、replay prerequisiteを検査するcreator report
- project、session、evidence、画像を読むloopback-only localhost UI
- process-local authenticated AI routeとnarrow Human Decision library boundary
- verified Ref snapshotから再構築できるSQLite ProjectionStore
- Linux x86_64 GNU向けv0.1.0 prerelease archiveとchecksum

実装範囲の詳細と根拠は[documentation index](./README.md#現在地)を参照する。

## 現在の利用対象

今すぐの評価対象は、CLIを扱えるtechnical creator、provenance／human-in-the-loop AIの
researcher・tool builder、Rust developerである。一般の画家、建築家、施工・修復担当、
デザイナーへ直接提供できるcapture/write UXはまだない。

## 未実装またはproduction blocker

- capture client、repeatable／calibrated capture workflow
- pixel registration、visual difference、physical change interpretation
- model／connector invocationとpre-execution OS sandbox／egress control
- write-capable localhost UI、Human review UI、maintenance UI
- HTTP/JWT／MFA、durable/distributed ACL・permit・publication fence
- organization／quorum／release approval、modified／partial adoption
- public multi-tenant cloud implementation、tenant isolation、operations
- SurrealDB adapterとbenchmark decision
- stable protocol/OID/archive compatibility commitment

## 配布上の現在地

| Item | Status |
|---|---|
| Public repository | Available |
| v0.1.0 GitHub prerelease | Available |
| Linux x86_64 GNU binary | Available; glibc 2.34+ |
| Source build from fixed tag | Available; Rust 1.88+ |
| SHA-256 release checksum | Available |
| Build provenance attestation | Added for future tagged builds; v0.1.0 predates it |
| crates.io / GHCR / OS packages | Intentionally unavailable in Stage 0 |
| Source use, Fork, and redistribution terms | Custom source-available license available; not open source |

## 次の優先順位

1. GitHub配布面のCI、license bundle、security reporting、metadata、community feedback導線を運用する。
2. creatorがCLIを意識せずcapture、review、exportできるlocal write sliceを作る。
3. fixed-point Observation datasetとpixel-level adapterを別contractとして検証する。
4. durable admission transactionを含むproduction control planeを実装する。

個別作業は公開Issueで、security-sensitiveな内容はprivate vulnerability reportingで管理する。
local path、未commit file、temporary cloud project ID等の作業環境snapshotは公開文書へ記録しない。

## Statusの更新方法

capabilityが変わる変更では、この文書、root README、
[documentation index](./README.md#現在地)の三つを同じPRで更新する。release時にはtag-pinned
release notesと[distribution guide](./distribution.md)のplatform／artifact情報も確認する。

## 次に読む

- [Installation](./install.md)
- [Usage guide](./usage_guide.md)
- [Runtime architecture](./runtime_architecture.md)
- [Security model](./security_model.md)
- [Stage 0 execution plan](./stage0_execution_plan.md)
- [Documentation index](./README.md)
