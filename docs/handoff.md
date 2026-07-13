# SynapseGit 作業引き継ぎ

更新日: 2026-07-14

この文書は、次の作業者が現在地を誤認せずに再開するための実装引き継ぎである。
規範仕様ではない。資料が食い違う場合は、[documentation index](./README.md#資料の位置づけ)に
記載した優先順位に従う。

## 1. Repository snapshot

- repository: `/home/o9oem/workspace/mine/temp/ai_git`
- working branch: `agent/archive-export-hardening`
- latest local head before the localhost application design slice: `23bfe94`
  (`docs: sync observation comparison workflow`)
- remote branch head: `cb21c45823bcd1f11031ca209157a33ee72816e2`
  (`docs: add current project handoff`)、`23bfe94`時点でlocal branchは5 commits ahead
- implementation baseline: `7f1fa96eba919b10401c6da8faaa717ff5d51c15`
  (`feat: harden archive export boundaries`)
- `origin/main`: `1249314`。上記branchは未mergeで、PRは未作成
- baseline検証: workspace 203 tests、Clippy `-D warnings`、Rustdoc `-D warnings`、
  formatting、fixture、documentation、Mermaid、diff checksが成功
- current slice commits:
  - `0b151d4 feat: add local creator pilot workflow`
  - `7579b58 feat: attach imported capture profiles`
  - `32b393d feat: add deterministic observation byte comparison`
  - `5f8b62a feat: report creator byte identity evidence`
  - `23bfe94 docs: sync observation comparison workflow`
- localhost application slice 1: architecture、versioned OpenAPI contract、contract verifier、docs syncを
  本handoffを含む次のlocal commitにする。commit後はremote baselineより6 commits aheadになる
- current検証: `cargo test --workspace --all-targets --locked` 218 tests、workspace Clippy `-D warnings`、
  Rustdoc `-D warnings`、format、Core fixture、localhost API contract（16 operations／39 schemas／137 refs）、
  official OpenAPI 3.1 schema、documentation link、Mermaid、diff checksが成功

`7f1fa96`ではarchive inventory／bytes／Ref／reflog／Tombstone／manifest、
distinct-head closure workをboundedにし、対応OSのarchive publicationをatomic no-replaceにした。
process-level export/update stressも追加した。詳細な契約は
[Local archive profile](../spec/core/v0.1/archive-profile.md)と
[Security model](./security_model.md)を参照する。

## 2. 一文で表す現在地

SynapseGitは、画像を含む制作物とAI／人の履歴を不変object graphとして保存・検証・移送できる
local Coreに加え、original／current／AI outputの3画像から手書きJSONなしで一sessionを記録する
**local single-creator Pilot**と、ordered primary Blob OIDを比較するdeterministic byte-identity baselineを持つ。
single-user／loopback-only画像applicationはarchitectureとHTTP contractまで決定した。一方、server／route／GUI、
capture、pixel-level registration／差分解析、実model実行、実利用者認証を備えたproduction creator applicationには
まだなっていない。

| 利用目標 | 現在の状態 |
|---|---|
| 開発者がlocal CLIとJSONを使ってCore round tripを試す | 利用可能 |
| embedding codeからAI proposal／Human Decision境界を使う | process-local Rust libraryとして利用可能 |
| 3画像から手書きJSONなしでAI proposal／Human Decision履歴を作る | local single-creator Pilotとして利用可能 |
| session timeline／process reportを表示しarchive restore後に再現する | local CLIとして利用可能 |
| original／currentのprimary Blob byte identityを記録する | `partial`なObservation baselineとして利用可能 |
| 画像registration／pixel差分／physical change解析を行う | 未実装 |
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
- `synapse-observation`によるordered Observation／CaptureProfile／全media検証とdeterministic primary Blob OID比較
- `synapse-creator`による3 opaque画像のingest、imported／reference-only CaptureProfile、Subject／Observation／
  Activity／専用software-tool Actor／byte-identity AnalysisResult／proposal／decision自動構成
- CLI `creator-run`によるadopt／reject／defer、AI／Human Application route、completion時`fsck`
- CLI `creator-report`によるcurrent lineage再検証、in-memory Projection timeline、両RefのAnalysis lineage、
  3画像OID／byte identity／review結果の表示。比較一式がないlegacy-shaped snapshotは
  `comparison=unavailable`として読取り可能だが、そのshapeは作成時期を証明しない
- creator archive／restore後に同じreport／Analysis OIDを再構築するprocess test

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
Subject、Actor、imported／reference-only CaptureProfile、Observation、Activity、AnalysisResult、ContextPack、
Policy、Grant、Tree、Commit、DecisionFeedbackを自動生成する。両Observationは同じprofileを参照するが、これは
repeatable／calibrated captureのclaimではない。AI agentとは別の`software_tool` Actorがoriginal→currentの
byte-identity Analysisをassertし、AnalysisResultとimplementation／configuration Blobをbase snapshotへ含める。
baseを継承するproposal／decision両Refから比較一式へ到達できる。

byte-identity adapterは両Observation、CaptureProfile、全mediaを検証するが、比較するのは各Observationで一意な
primary Blob OIDだけである。画像をdecodeせず、EXIF、pixel、registration、difference analysisを扱わない。
成功時も`partial`／`byte_identity_only`であり、bytesの同一／相違からvisualまたはphysical changeを推論しない。
fileから外部撮影／実行時刻を得たとは見なさず、Observation `capture_time`とActivity `valid_time`は`unknown`にする。

AI outputはcommandがmodelを実行して作るものではない。trusted local integrationが用意したfileを、fixed local
Pilot Authenticator／profile／prepared Executorを使うApplication AI routeからproposalへ公開し、same-instance
admitted handleをHuman routeへ渡す。`--creator`はself-declaredな表示名で、本人確認credentialではない。
EntityIdはrunごとにOSの暗号学的乱数から生成するsession-local IDであり、Subject extensionのPilot-private
manifestへcore IDを保存し、comparison tool／analysis IDは保存されたseries IDから決定的に再導出する。
同じ人のsession間identityではないが、reportとarchive restoreは同じIDを復元できる。`adopt`だけがproposal
snapshotを選択し、`reject`／`defer`はbase snapshotを維持する。reportでは
`proposal_attributed_to_agent`、`ai_output_source=caller_supplied`、`reviewed_by_human`を分け、
`selected=true`はadoptだけである。DecisionFeedbackの既定はreason `unspecified`、private visibility、
training use prohibitedである。

`creator-report`は一つのconsistent Ref snapshotから両creator headを解決し、base／proposal／decision snapshotと
current proposal／DecisionFeedback／AI Activityのlineageを再検証する。同じsnapshotからdisposable in-memory
Projectionをrebuildし、byte-identity Analysisのordered input、tool Actor、implementation／configuration、
availability-only replay readiness、両Ref reachabilityとtimelineを検査して、最後に`fsck`する。比較一式のない
legacy-shaped creator snapshotは`comparison=unavailable`として読めるが、そのshapeは作成時期を証明しない。
一部だけがあるsnapshotは拒否する。
timelineは各stageのrun内で単調増加するrecording
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
- localhost image applicationのsafe facade、Axum／Askama、OpenAPI、browser security、8 implementation slicesは
  [architecture](./localhost_application_architecture.md)で決定済み。実server／route／GUIは未実装
- byte-identity reportは実装済み。実capture、pixel-level画像比較、実model／connector実行、継続session編集は未実装
- ペイントツール、ファイル監視、実利用者Pilotとbenefit measurementは未実装

現在の実装は価値仮説を試す最小local経路であり、制作現場へ配布できるproduction applicationではない。

### B. 画像比較を提供するためのObservation層

- ordered Observation、imported CaptureProfile、全media validation、primary Blob OID comparisonはRust baselineとして実装済み
- 成功結果を`partial`／`byte_identity_only`に限定し、visual／physical changeをclaimしない
- Painting control／Building validation dataset
- repeatable／calibrated CaptureProfileを使うcapture
- pixel-level image registrationとPython adapter
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
最初のlocalhost applicationはこのproduction service層を完成させるものではなく、IPv4 loopback固定、single-user、
same-process handle、OS user／directory permissionを信頼する境界に限定する。

### D. Protocol／運用hardening

- 第二の独立production実装による`sg-oid-v1` freeze gate
- write-boundary process-kill fault injection
- archive staging／ObjectStore temporary fileの安全なstartup cleanup
- CIのMSRV／OS matrix、large-store benchmark、運用監視
- optional SurrealDB adapterと全8-query比較
- `creator-report`は現binaryが計算するbyte-identity implementation／configuration OIDを厳密に要求する。
  将来adapter sourceまたはconfigurationを変更すると旧session reportを読めなくなるため、version別digest allowlist、
  migration、または明示的なadapter version bumpの方針が必要

startup cleanupは、pathnameだけを見て古いfileを削除すると別writerのdataを消すABA raceがある。
atomicな所有権公開、fd/path identity、lifetime-wide coordinationを設計せずに再導入しない。

## 6. 今回実装したvertical sliceと次の優先事項

次のlocal経路を`creator-run`、`creator-report`、既存archive commandで実装した。

```text
original / current / caller-supplied AI outputを取り込む
  -> imported CaptureProfile / Subject / Observation / Activityを自動作成
  -> dedicated software_toolでordered byte-identity AnalysisResultをbase snapshotへ記録
  -> Application AI routeでproposalを公開
  -> Application Human routeでadopt / reject / deferを記録
  -> current lineageと両RefのAnalysis lineageを検証してProjection timeline / process reportを表示
  -> fsck
  -> export / restore
  -> restored Refs + CASから同じreportを再構築
```

このsliceは次を満たす。

- 一つのcommand flowから3画像を取り込み、手書きJSONなしで履歴objectを作る
- original、current AI input、AI output、proposal、人のreviewをOIDで相互に辿る
- imported／reference-only CaptureProfileと専用software toolを作り、AI agentのprovenanceと分離する
- original→currentのprimary Blob byte identityを`partial`／`byte_identity_only`として記録する
- AnalysisResult／implementation／configurationをbase snapshotへ束縛し、proposal／decision両Refから辿る
- caller-supplied sourceとagent attribution、human reviewを分け、`selected=true`をadoptだけに限定する
- 一つのRef snapshotでcurrent proposal／Feedback／base・proposal・decision snapshot lineageと`fsck`を再検証する
- archive restore後のreport／Analysis OID equalityをcreator／CLI process testで検査する
- comparison一式を持たないlegacy-shaped snapshotを、作成時期を推測せず`comparison=unavailable`として読む
- Observation capture timeとActivity valid timeを根拠なく捏造せず`unknown`にする
- timelineのrecording timestampをcapture／AI execution timeとして扱わない
- core session-local EntityIdをSubject manifestから復元し、comparison IDをseries IDから再導出する。
  どちらもcross-session identityとは扱わない
- incomplete sessionをcreate-only conflictとして保全し、自動resume／cleanupしない

次の優先事項は[localhost application architecture](./localhost_application_architecture.md)のslice 2である。
`synapse-local-service`のversioned read DTO／exact project catalogと、`synapse-local-http`のloopback／Host／Origin／token
boundaryを追加し、project status、Refs、reflog、creator session list／report／imageのread-only APIを実装する。
transport crateはCore／SQLiteへ直接依存させず、`update-ref`、raw object、authority／permit routeが存在しないことをtestする。
先に、Ref snapshotと`LIMIT + 1` reflog pageを同じSQLite read transactionで返すread API、caller-supplied snapshotから
reportとProjection fingerprintを同時に返すcreator API、64 MiBでfail-closedするverified Blob readerを追加する。

その後server-rendered dashboard、3-file proposal-only begin、byte evidence view、same-process Human review、maintenance job、
incomplete diagnostics／packaging／E2Eを別commitで進める。Workstream Cと実model／connectorは並行候補だが、現在の
byte identityをvisual／physical画像差分の代用にしない。

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
node scripts/verify_local_api.mjs
node scripts/verify_docs.mjs
node scripts/verify_mermaid.mjs
git diff --check
```

実装状態を変えた場合は、root `README.md`、[documentation index](./README.md)、
[localhost application architecture](./localhost_application_architecture.md)、[Stage 0 execution plan](./stage0_execution_plan.md)、
[使用ガイド](./usage_guide.md)の状態表記を同期する。
