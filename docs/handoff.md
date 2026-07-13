# SynapseGit 作業引き継ぎ

更新日: 2026-07-13

この文書は、次の作業者が現在地を誤認せずに再開するための実装引き継ぎである。
規範仕様ではない。資料が食い違う場合は、[documentation index](./README.md#資料の位置づけ)に
記載した優先順位に従う。

## 1. Repository snapshot

- repository: `/home/o9oem/workspace/mine/temp/ai_git`
- working branch: `agent/archive-export-hardening`
- implementation baseline: `7f1fa96eba919b10401c6da8faaa717ff5d51c15`
  (`feat: harden archive export boundaries`)
- baseline remote: `origin/agent/archive-export-hardening` と一致
- `origin/main`: `1249314`。上記branchは未mergeで、PRは未作成
- baseline検証: workspace 203 tests、Clippy `-D warnings`、Rustdoc `-D warnings`、
  formatting、fixture、documentation、Mermaid、diff checksが成功

`7f1fa96`ではarchive inventory／bytes／Ref／reflog／Tombstone／manifest、
distinct-head closure workをboundedにし、対応OSのarchive publicationをatomic no-replaceにした。
process-level export/update stressも追加した。詳細な契約は
[Local archive profile](../spec/core/v0.1/archive-profile.md)と
[Security model](./security_model.md)を参照する。

## 2. 一文で表す現在地

SynapseGitは、画像を含む制作物とAI／人の履歴を不変object graphとして保存・検証・移送できる
**local Core／developer integration基盤**である。一方、画家が画像を取り込むだけで履歴を自動生成し、
AI案を確認・採否できる**creator-facing application**にはまだなっていない。

| 利用目標 | 現在の状態 |
|---|---|
| 開発者がlocal CLIとJSONを使ってCore round tripを試す | 利用可能 |
| embedding codeからAI proposal／Human Decision境界を使う | process-local Rust libraryとして利用可能 |
| 画像と、別途作成したAI Activity／生成物を同じ履歴へ格納する | 利用可能 |
| 画家が手作業JSONなしで制作履歴を残す | 未実装 |
| 画像registration／差分解析を行う | 未実装 |
| untrustedな複数利用者へnetwork serviceとして提供する | production境界が未実装 |

## 3. 実装済みの中心

- strict JSON、canonical bytes、domain-separated OID、20 concrete schemas
- filesystem CAS、typed closure、Tombstone availability、`fsck`
- SQLite Ref compare-and-swapと完全なreflog
- validated ingest、checksum付きdirectory export／restore
- `put-blob → put-record → build-tree → commit → update-ref → fsck → export → restore`
  のlocal CLI round trip
- `CreativeAiRuntime`によるproposal-only admissionと、process-local authenticated one-shot AI route
- admitted proposalだけを対象とするnarrow Human Decision route
- current Ref closureから再構築するdisposable SQLite ProjectionStore

CLIのcommandと制約は[CLI reference](./cli_reference.md)、実行例は[Quickstart](./quickstart.md)を参照する。

## 4. 画像とAI履歴の正確な境界

画像はJPEG、PNG、RAW等の意味をCoreが解釈せず、opaque Blobとしてそのまま保存できる。
Observationの`media_refs`、Activityの`input_refs`／`output_refs`、Tree、CommitがBlob OIDを
関連付ける。AI Activityはagent、responsible principal、ContextPack、DelegationGrant、capability、
入力、出力、statusを記録でき、人はproposalに対して採用、却下、保留、実験扱いを記録できる。

これは画像のEXIF等へ履歴を書き込む方式ではない。原画像を変更せず、byte identityを持つBlobへ
外付けのobject graphを結び付ける方式である。OIDが証明するのはbyte identityであり、作者性、真実、
撮影時刻、著作権、許諾を自動証明しない。

現在のCLIはBlobと、利用者が用意したRecord／Tree／Commitを格納できるが、制作sessionやAI会話から
それらを自動生成しない。また、CLIの`update-ref`はtrusted operator primitiveであり、
`synapse-application`のAI／Human admissionを通らない。untrusted callerへ公開してはならない。

## 5. 残作業は何のためか

残作業はすべてが任意の「豪華機能」ではない。どの利用目標を完成とするかで必須範囲が変わる。

### A. Creatorへ便益を届ける層

- 画像取込み、Subject／Observation／Activity作成の自動化
- AI実行、proposal、人の採否を一続きにするapplication CLIまたはUI
- 履歴timeline、比較、制作process report、handoff出力
- ペイントツール、ファイル監視、model／connectorとのintegration

Coreの能力を画家が受け取るにはこの層が必要であり、単なる装飾ではない。

### B. 画像比較を提供するためのObservation層

- Painting control／Building validation dataset
- image registrationとPython adapter
- `comparable`／`partial`／`incomparable` reason code
- `changed`／`unchanged`／`ambiguous`／`unobservable` mask
- 照明差、遮蔽、blur、露出不良、registration失敗を含む評価report

「画像を保存する」だけなら不要だが、「物理変化や差分候補を提示する」なら必須である。
欠測や比較不能を「変化なし」へ変換しない。

### C. Production service層

- HTTP／JWT／MFA、credential store、TLS、rate limiting
- durable／distributed ACL・profile・permitとmulti-process fence
- AI ExecutorのOS sandbox、connector／egress／SSRF制御、Grant revocation
- multi-project CAS membership、authenticated Projection route
- organization／quorum、release、modified／partial adoption workflow

localなtrusted developer Pilotでは必須ではないが、untrusted caller、複数利用者、公開serviceでは必須である。

### D. Protocol／運用hardening

- 第二の独立production実装による`sg-oid-v1` freeze gate
- write-boundary process-kill fault injection
- archive staging／ObjectStore temporary fileの安全なstartup cleanup
- CIのMSRV／OS matrix、large-store benchmark、運用監視
- optional SurrealDB adapterと全8-query比較

startup cleanupは、pathnameだけを見て古いfileを削除すると別writerのdataを消すABA raceがある。
atomicな所有権公開、fd/path identity、lifetime-wide coordinationを設計せずに再導入しない。

## 6. 次回の推奨vertical slice

creator便益を最短で検証するなら、最初に次の一経路を完成させる。

```text
original / current imageを取り込む
  -> Subject / Observationを自動作成
  -> AI Activityとproposal outputを記録
  -> 人がadopt / reject / deferを選ぶ
  -> timeline / process reportを表示
  -> fsckしてarchive / restore
```

最初の版では自動画像差分を必須にせず、AIとの制作履歴を手作業JSONなしで残せる価値を先に検証できる。
その後、Workstream Cのfixed-viewpoint Observation adapterを接続する。formalなStage 0 exitを優先する場合は、
[Stage 0 execution plan](./stage0_execution_plan.md)のprotocol freeze、Observation Pilot、benefit measurement、
Projection比較を並行して進める。

このcreator sliceの最低完了条件は次のとおりである。

- 一つのcommand flowまたは画面から画像を取り込み、手書きJSONなしで履歴objectを作れる
- original、AI input、AI output、人の判断がOIDで相互に辿れる
- `generated_by AI`と`selected/rejected by human`を混同しない
- reportとarchive restore後の履歴が同じOIDで再現される
- CLI／application process testと利用者向け手順が追加される

## 7. 再開時の確認

```bash
git fetch origin
git switch agent/archive-export-hardening
git status --short --branch
git log -5 --oneline --decorate

cargo fmt --all -- --check
cargo test --workspace --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
RUSTDOCFLAGS='-D warnings' cargo doc --workspace --no-deps --locked
node scripts/verify_core_fixtures.mjs
node scripts/verify_docs.mjs
node scripts/verify_mermaid.mjs
git diff --check
```

実装状態を変えた場合は、root `README.md`、[documentation index](./README.md)、
[Stage 0 execution plan](./stage0_execution_plan.md)、[使用ガイド](./usage_guide.md)の状態表記を同期する。
