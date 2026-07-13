# SynapseGit 作業引き継ぎ

更新日: 2026-07-13

この文書は、次の作業者が現在地を誤認せずに再開するための実装引き継ぎである。
規範仕様ではない。資料が食い違う場合は、[documentation index](./README.md#資料の位置づけ)に
記載した優先順位に従う。

## 1. Repository snapshot

- repository: `/home/o9oem/workspace/mine/temp/ai_git`
- working branch: `agent/archive-export-hardening`
- published branch head: `cb21c45823bcd1f11031ca209157a33ee72816e2`
  (`docs: add current project handoff`)、`origin/agent/archive-export-hardening`と一致
- implementation baseline: `7f1fa96eba919b10401c6da8faaa717ff5d51c15`
  (`feat: harden archive export boundaries`)
- `origin/main`: `1249314`。上記branchは未mergeで、PRは未作成
- baseline検証: workspace 203 tests、Clippy `-D warnings`、Rustdoc `-D warnings`、
  formatting、fixture、documentation、Mermaid、diff checksが成功
- current local slice: `synapse-creator`、CLI `creator-run`／`creator-report`、creator／CLI
  process tests、関連documentationをpublished baseline後のlocal commitとして実装済み。branchはremoteよりaheadである
- current検証: workspace 211 tests、Clippy `-D warnings`、Rustdoc `-D warnings`、format、fixture、
  documentation、Mermaid、diff checksが成功

`7f1fa96`ではarchive inventory／bytes／Ref／reflog／Tombstone／manifest、
distinct-head closure workをboundedにし、対応OSのarchive publicationをatomic no-replaceにした。
process-level export/update stressも追加した。詳細な契約は
[Local archive profile](../spec/core/v0.1/archive-profile.md)と
[Security model](./security_model.md)を参照する。

## 2. 一文で表す現在地

SynapseGitは、画像を含む制作物とAI／人の履歴を不変object graphとして保存・検証・移送できる
local Coreに加え、original／current／AI outputの3画像から手書きJSONなしで一sessionを記録する
**local single-creator Pilot**を持つ。一方、capture、画像解析、実model実行、実利用者認証、GUIを備えた
production creator applicationにはまだなっていない。

| 利用目標 | 現在の状態 |
|---|---|
| 開発者がlocal CLIとJSONを使ってCore round tripを試す | 利用可能 |
| embedding codeからAI proposal／Human Decision境界を使う | process-local Rust libraryとして利用可能 |
| 3画像から手書きJSONなしでAI proposal／Human Decision履歴を作る | local single-creator Pilotとして利用可能 |
| session timeline／process reportを表示しarchive restore後に再現する | local CLIとして利用可能 |
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
- `synapse-creator`による3 opaque画像のingestとSubject／Observation／Activity／proposal／decision自動構成
- CLI `creator-run`によるadopt／reject／defer、AI／Human Application route、completion時`fsck`
- CLI `creator-report`によるcurrent lineage再検証、in-memory Projection timeline、3画像OIDとreview結果の表示
- creator archive／restore後に同じreport／OIDを再構築するprocess test

CLIのcommandと制約は[CLI reference](./cli_reference.md)、creator実行例は[使用ガイド](./usage_guide.md#手書きjsonなしのlocal-creator-pilot)を参照する。

## 4. 画像とAI履歴の正確な境界

画像はJPEG、PNG、RAW等の意味をCoreが解釈せず、opaque Blobとしてそのまま保存できる。
Observationの`media_refs`、Activityの`input_refs`／`output_refs`、Tree、CommitがBlob OIDを
関連付ける。AI Activityはagent、responsible principal、ContextPack、DelegationGrant、capability、
入力、出力、statusを記録でき、人はproposalに対して採用、却下、保留、実験扱いを記録できる。

これは画像のEXIF等へ履歴を書き込む方式ではない。原画像を変更せず、byte identityを持つBlobへ
外付けのobject graphを結び付ける方式である。OIDが証明するのはbyte identityであり、作者性、真実、
撮影時刻、著作権、許諾を自動証明しない。

CLIの`creator-run`はoriginal、current、caller-supplied AI outputをopaque Blobとして格納し、必要な
Subject、Actor、Observation、Activity、ContextPack、Policy、Grant、Tree、Commit、DecisionFeedbackを自動生成する。
画像をdecodeせず、EXIF、pixel、registration、difference analysisを扱わない。fileから外部撮影／実行時刻を
得たとは見なさず、Observation `capture_time`とActivity `valid_time`は`unknown`にする。

AI outputはcommandがmodelを実行して作るものではない。trusted local integrationが用意したfileを、fixed local
Pilot Authenticator／profile／prepared Executorを使うApplication AI routeからproposalへ公開し、same-instance
admitted handleをHuman routeへ渡す。`--creator`はself-declaredな表示名で、本人確認credentialではない。
EntityIdはrunごとにOSの暗号学的乱数から生成するsession-local IDであり、Subject extensionのPilot-private
manifestへ保存する。同じ人のsession間identityではないが、reportとarchive restoreはmanifestから同じIDを
復元できる。`adopt`だけがproposal snapshotを選択し、`reject`／`defer`はbase snapshotを維持する。reportでは
`proposal_attributed_to_agent`、`ai_output_source=caller_supplied`、`reviewed_by_human`を分け、
`selected=true`はadoptだけである。DecisionFeedbackの既定はreason `unspecified`、private visibility、
training use prohibitedである。

`creator-report`は一つのconsistent Ref snapshotから両creator headを解決し、base／proposal／decision snapshotと
current proposal／DecisionFeedback／AI Activityのlineageを再検証する。同じsnapshotからdisposable in-memory
Projectionをrebuildしてtimelineを作り、最後に`fsck`する。timelineは各stageのrun内で単調増加するrecording
timestampを`recorded_at` fallbackとして表示し、撮影時刻やAI execution timeを意味しない。
一般的なauthenticated Projection routeではない。archive／restoreは別commandだが、restore後のreport equalityを
process testで検証する。

creator sessionはcreate-onlyである。base Ref公開後かつHuman Decision前のfailureは
`creator_session_incomplete`を残す。Decision publication後のfailureはcomplete sessionを残し得る。
Pilotはどちらも自動resume／cleanupも上書きもしないため、callerはcurrent Refsを診断する。

low-level CLIの`update-ref`は引き続きtrusted operator primitiveであり、`synapse-application`のAI／Human
admissionを通らない。`creator-run`を含め、現在のCLIをuntrusted callerへ公開してはならない。

## 5. 残作業は何のためか

残作業はすべてが任意の「豪華機能」ではない。どの利用目標を完成とするかで必須範囲が変わる。

### A. Creatorへ便益を届ける層

- 3 file取込み、Subject／Observation／Activity作成、proposal、人のadopt／reject／deferはlocal CLIで実装済み
- current lineage検証、履歴timeline、text process report、`fsck`、archive restore再現はlocal Pilotで実装済み
- 実capture、画像比較、実model／connector実行、継続session編集、GUIは未実装
- ペイントツール、ファイル監視、実利用者Pilotとbenefit measurementは未実装

現在の実装は価値仮説を試す最小local経路であり、制作現場へ配布できるproduction applicationではない。

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

## 6. 今回実装したvertical sliceと次の優先事項

次のlocal経路を`creator-run`、`creator-report`、既存archive commandで実装した。

```text
original / current / caller-supplied AI outputを取り込む
  -> Subject / Observation / Activityを自動作成
  -> Application AI routeでproposalを公開
  -> Application Human routeでadopt / reject / deferを記録
  -> current lineageを検証してProjection timeline / process reportを表示
  -> fsck
  -> export / restore
  -> restored Refs + CASから同じreportを再構築
```

このsliceは次を満たす。

- 一つのcommand flowから3画像を取り込み、手書きJSONなしで履歴objectを作る
- original、current AI input、AI output、proposal、人のreviewをOIDで相互に辿る
- caller-supplied sourceとagent attribution、human reviewを分け、`selected=true`をadoptだけに限定する
- 一つのRef snapshotでcurrent proposal／Feedback／base・proposal・decision snapshot lineageと`fsck`を再検証する
- archive restore後のreport／OID equalityをcreator／CLI process testで検査する
- Observation capture timeとActivity valid timeを根拠なく捏造せず`unknown`にする
- timelineのrecording timestampをcapture／AI execution timeとして扱わない
- session-local EntityIdをSubject manifestから復元し、cross-session identityとは扱わない
- incomplete sessionをcreate-only conflictとして保全し、自動resume／cleanupしない

次の優先事項は、実利用者にこのCLIを試してもらい、記録負担、判断再発見、report／handoff時間を測ることである。
並行してWorkstream Cのfixed-viewpoint Observation dataset／adapterへ進む。自動画像差分をcreator Pilotへ接続する前に、
照明差、遮蔽、blur、露出不良、registration失敗を評価し、比較不能を「変化なし」にしない。
実model／connector／paint tool integration、継続session UXもcreator側の次候補である。

formalなStage 0 exitには、[Stage 0 execution plan](./stage0_execution_plan.md)の第二独立実装によるprotocol freeze、
Observation Pilot、creator benefit measurement、SurrealDBを含むProjection比較が引き続き必要である。

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
