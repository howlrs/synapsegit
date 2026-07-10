# SynapseGit Core runtime architecture

Status: Stage 0 decision draft

Decision date: 2026-07-11

## 結論

Core実装言語は**Rust**を第一候補にする。ただし正本をSurrealDBへ閉じ込めず、Gitライクな不変objectをfilesystem/object storage上のCASに置く。ローカルのRef・reflog・既定indexはSQLite、共有serviceではPostgreSQLを候補とし、SurrealDBは消去・再構築できる`ProjectionStore` adapterとして並行評価する。

```text
Rust synapse-core（DB非依存）
  strict parser / canonical bytes / OID / schema
  tree / commit / closure / fsck / export / restore
                |
       +--------+---------+
       |                  |
 ObjectStore           RefStore
 正本                  可変な入口 + reflog
 local: filesystem     local: SQLite
 cloud: object store   cloud: PostgreSQL
       |
 ProjectionStore（全消去して正本から再構築可能）
 default: SQLite / optional: SurrealDB
```

この境界では、DBを交換してもOID、Commit DAG、archiveは変わらない。SurrealDBの採否は作品履歴を賭ける不可逆な選択ではなく、検索・グラフ探索の便益で判断できる可逆な選択になる。

## Gitから踏襲するもの

Gitらしさの中核は使用言語やRDBではない。Git自身がcontent-addressable filesystemとしてobjectを扱い、Ref更新では現在値がold OIDと一致した場合だけnew OIDへ進める。SynapseGitも次を踏襲する。

- canonical bytesに対するcontent address
- Blob / Record / Tree / Commitの不変object
- first-parentを持つCommit DAG
- `expected_head`付きRef compare-and-swap
- reflog、fsck、到達closure、export/restore
- objectを先に永続化し、closure確認後にRefを進める書込み順序

参考: [Git objects](https://git-scm.com/book/en/v2/Git-Internals-Git-Objects.html)、[git-update-ref](https://git-scm.com/docs/git-update-ref.html)

## 候補比較

| 構成 | local/offline | graph探索 | Ref CAS | 復元性 | 判断 |
|---|---:|---:|---:|---:|---|
| Rust + filesystem CAS + SQLite | ◎ | ○。recursive CTE | ◎。transaction | ◎。index再構築可 | **Stage 1既定** |
| Rust + filesystem CAS + SurrealDB | ◎。embedded可 | ◎。型付きedge・arrow traversal | ○。競合試験が必要 | ○。projection限定なら◎ | **並行spike** |
| Rust + object CAS + PostgreSQL | △。server前提 | ○。recursive CTE | ◎ | ◎ | 共有service候補 |
| TypeScriptだけでCore | ○ | DB次第 | DB次第 | △ | UI/SDKには採用、OID正本には不採用 |
| PythonだけでCore | ○ | DB次第 | DB次第 | △ | adapter/AI workerには採用、OID正本には不採用 |

SQLiteは公式にversion-control system、media editing、CAD等のapplication file formatを用途として挙げ、ACID transactionとgraphを辿れるrecursive CTE、online backupを持つ。最初のローカル実装に必要な性質が少ない運用部品で揃う。[SQLite appropriate uses](https://www.sqlite.org/whentouse.html)、[transactions](https://www.sqlite.org/transactional.html)、[recursive CTE](https://sqlite.org/lang_with.html)、[backup](https://www.sqlite.org/backup.html)

SurrealDBはRust SDKからin-memoryまたはfile-backed embedded databaseとして実行でき、relationをmetadata付きedgeとして保持・探索できる。Actor–Activity–Observation–Claim–ContextPackの横断には明確な適性がある。[Rust embedding](https://surrealdb.com/docs/reference/rust/embedding)、[graph relations](https://surrealdb.com/docs/learn/data-models/graph/creating-relations)

一方、2026-07時点の公式資料ではSurrealKVはbetaで、保守的なon-disk productionにはRocksDBが推奨される。また2.xから3.xにはmanual対応を含むbreaking changesがある。したがってDB内部表現をarchive正本にせず、projectionを再生成できることを採用条件にする。[storage engines](https://surrealdb.com/docs/build/embedding/storage-engines)、[2.x to 3.x migration](https://surrealdb.com/docs/build/migrating/from-old-surrealdb-versions/2x-to-3x)

## 言語の責任分担

### Rust: OID決定権を持つCore

- strict UTF-8/JSON parser、duplicate keyとnumber token検査
- canonical serializer、SHA-256、OID
- filesystem CAS、streaming Blob ingest、atomic rename
- schema/semantic validator、Commit closure、Ref CAS
- CLI、fsck、archive export/restore
- C ABI/WASMまたはservice APIを通した他言語連携

OIDを生成する実装を一つに絞るのは、他言語を排除するためではなく、初期段階でcanonicalizationの分裂を防ぐためである。golden fixtureが安定した後、独立した第二実装で相互検証する。

### TypeScript: Creator-facing application

- desktop/web UI、capture review、Diff viewer
- project workflow、branch/merge interaction
- Coreが返したOIDを扱うclient SDK

### Python: mediaとCreative AI adapter

- image registration、mask、photometric analysis
- BIM/CAD/画像adapter
- model provider連携、ContextPack consumer

Python/TypeScript workerはArtifactやAnalysis bodyをCoreへ提出するが、自前serializerのdigestを正本にしない。Coreがcanonicalize・validateしてOIDを返す。

## 永続化の書込み境界

local writeは次の順序に固定する。

1. Blobまたはcanonical objectを同一filesystemのtemporary fileへstreamする。
2. hashとschema/semantic ruleを検証する。
3. fileと必要なdirectoryをflushし、OID pathへatomic renameする。
4. index projectionを追加する。失敗しても再構築可能とする。
5. candidate Commitのclosureを検証する。
6. Refとreflogを同じSQLite transactionでcompare-and-swap更新する。

途中停止時は未到達objectが残り得るが、公開済みRefが不完全なCommitを指すことはない。garbage collectionはgrace period後の別operationとする。

## SurrealDB採用spike

Stage 1の最初に、同じCASを入力としてSQLiteとSurrealDBへprojectionを作り、次の代表queryを両方で実装する。

1. CommitからBlobまでのclosureと欠落理由
2. SubjectのObservation/Activity timeline
3. ObservationからCaptureProfile・Calibrationへの到達
4. Analysisの入力、adapter、派生物、再実行候補
5. ClaimのEvidence、代替Claim、actor別Reaction
6. AI ArtifactからAIRun、ContextPack、Policy、DelegationGrantへの到達
7. 削除対象からpreview・mask・embedding等の派生物列挙
8. Creator/AIの`generated_by`, `selected_by`, `modified_by`, `approved_by`分離

比較するのは単純benchmarkだけではない。

- 10万edge規模でのp50/p95 latencyと起動時間
- queryとprojection adapterの実装量・理解しやすさ
- concurrent Ref CAS race testの正しさ
- schema migrationと旧projectionの破棄・再構築時間
- empty storeへのrestore後に全OIDとquery結果が一致すること
- binary size、memory、backup、障害復旧の運用負荷

SurrealDBは、横断queryの実装単純性または性能がSQLiteより実測で明確に優れ、かつprojection再構築・競合試験を通る場合に既定へ昇格する。それまでは価値あるoptional adapterとして扱う。

## Stage 1 repository案

```text
crates/
  synapse-canonical   strict parser / canonical bytes / OID
  synapse-schema      JSON Schema + semantic validation
  synapse-cas         filesystem and object-store traits
  synapse-graph       tree / commit / closure
  synapse-ref         RefStore trait / reflog / CAS
  synapse-sqlite      local RefStore + ProjectionStore
  synapse-surreal     optional ProjectionStore spike
  synapse-cli         put / commit / fsck / export / restore
workers/
  image-analysis/     Python adapter
apps/
  desktop/            TypeScript UI
```

最初の実装単位は`put-object → verify OID → build tree → commit → CAS ref → fsck → export/restore`の縦切りとする。SurrealDB導入はこの縦切りを遅らせず、projection traitの背後で並行検証する。
