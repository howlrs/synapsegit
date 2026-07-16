# SynapseGit project status

Audience: preview evaluators、contributors、maintainers
Status: public project snapshot
Applies to: current main; tagged v0.2.0 differences are noted below
Last verified: 2026-07-16

SynapseGit Coreは**Stage 0 draft**である。v0.2.0は、local repository、bounded creator
Pilot、localhost import／reviewを評価するためのprereleaseであり、production-readyな
creator applicationやmulti-user serviceではない。

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
- boundedな三file importとsame-process Human reviewを行うlocalhost creator UI
- current creator Ref／headと推奨actionを表示するread-only incomplete-session diagnostics
- exact project確認、server-fixed limit、process-local job pollingを持つlocalhost `fsck` UI
- process-local authenticated AI routeとnarrow Human Decision library boundary
- verified Ref snapshotから再構築できるSQLite ProjectionStore
- Linux x86_64 GNU向けv0.2.0 prerelease archive、checksum、build attestation

実装範囲の詳細と根拠は[documentation index](./README.md#現在地)を参照する。

## 現在の利用対象

今すぐの評価対象は、CLIを扱えるtechnical creator、provenance／human-in-the-loop AIの
researcher・tool builder、Rust developerである。一般の画家、建築家、施工・修復担当、
デザイナーへそのまま提供できるcapture／継続編集UXにはまだ達していない。current `main`の
localhost UIは三file importと単一proposalのreviewを行えるが、AI outputはcaller-suppliedで、
pending reviewはprocess restartを越えて復元できない。current mainではrestart後等のincomplete sessionを
read-onlyで診断し、明示確認したbounded `fsck`をbackground jobとしてpollできる。表示したRef／headから
authorityを再構築せず、自動resume／cleanupも行わない。job stateと`last_fsck`はprocess-localである。

## 未実装またはproduction blocker

- capture client、repeatable／calibrated capture workflow
- pixel registration、visual difference、physical change interpretation
- model／connector invocationとpre-execution OS sandbox／egress control
- localhostのarchive list・export・restore API／UI
- restart-durable review authority、automatic resume／cleanup、継続session編集
- HTTP/JWT／MFA、durable/distributed ACL・permit・publication fence
- organization／quorum／release approval、modified／partial adoption
- public multi-tenant cloud implementation、tenant isolation、operations
- SurrealDB adapterとbenchmark decision
- stable protocol/OID/archive compatibility commitment

## 配布上の現在地

| Item | Status |
|---|---|
| Public repository | Available |
| v0.2.0 GitHub prerelease | Available |
| Linux x86_64 GNU binary | Available; glibc 2.34+ |
| Source build from fixed tag | Available; Rust 1.88+ |
| SHA-256 release checksum | Available |
| Build provenance attestation | Available for the v0.2.0 archive |
| crates.io / GHCR / OS packages | Intentionally unavailable in Stage 0 |
| Source use, Fork, and redistribution terms | Custom source-available license available; not open source |

`v0.2.0`の`SynapseGit Local` binaryは上記の三file import／reviewを含む。review authorityは
process-localであり、process restartを越えたpending reviewの再開はできない。current mainで追加した
dedicated read-only incomplete-session diagnosticsとbounded browser `fsck`はtagged v0.2.0 binaryには
含まれない。

## 次の優先順位

1. 実装済みlocalhost import／review／diagnostics／bounded `fsck`を、archive list／export／restoreへ拡張する。
2. fixed-point Observation datasetとpixel-level adapterを別contractとして検証する。
3. durable admission transactionを含むproduction control planeを実装する。
4. 追加platformの再現可能なbuild／artifact smokeを整備する。

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
