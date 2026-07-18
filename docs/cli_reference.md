# SynapseGit CLI reference

`synapse` is a local Stage 0 interface for object ingestion, Ref updates, integrity checks, directory archive round trips,
and one bounded single-creator Pilot. It is not a network client or production authorization boundary. The Rust
workspace provides process-local authenticated AI and admitted-proposal-bound narrow Human routes
in `synapse-application`, plus `synapse-core::CreativeAiRuntime` and `HumanDecisionRuntime`.
`creator-run` uses both routes internally with fixed local Pilot identities, a prepared Executor, and a
caller-supplied AI output; it is not a general proposal／decision publication API or real-user authentication.
The workspace also provides `synapse-projection::SqliteProjectionStore`. `creator-report` uses it for one
bounded session timeline and byte-identity lineage, but the CLI has no general projection rebuild or query command.
The separate `synapse-present` companion reads completed creator history and generates a local, derived publication
bundle. It does not change any `synapse` command or turn the Core archive command into a presentation export.

Status: **implemented at Core v0.1 / Stage 0 draft**

## Build and help

```bash
cargo build -p synapse-cli --locked
target/debug/synapse --help
target/debug/synapse --version
```

`cargo run` 経由では `--` を挟む。

```bash
cargo run -p synapse-cli -- --help
```

成功は exit code 0、全 error は現在 exit code 1。error は stderr の先頭に `<code>:` を付ける。
usage error の場合は usage 全文も stderr に出す。
`--version`、`-V`、`version` は `synapse <package-version>` を stdout に出して exit code 0 で終了する。

## `synapse-present` companion CLI

`synapse-present`は既存CASをread-onlyで扱い、Ref SQLiteのstable private copyから取得した一つの
bounded Ref snapshotを使って、人向けとmachine向けのlocal publication bundleを同時に生成する。
Coreの`init`、`creator-report`、`export`を含む既存command、stdout／stderr、exit code、archive formatは
変更しない。

```bash
cargo build -p synapse-publication --bin synapse-present --locked
target/debug/synapse-present --help
target/debug/synapse-present --version
```

```text
synapse-present export <repo> <output-dir> [--session <id>]
  [--presentation <presentation.toml>] [--public]
  [--target <synapse|github> | --synapse | --github]

synapse-present preview <bundle-dir>
```

### `export <repo> <output-dir> [options]`

source repositoryを作成せず、CAS、Refs、reflogへ書き込まずにbundleを新規生成する。export前に
`synapse-local`と同repositoryへ書く全CLI／processを停止する。Ref databaseはcheckpoint済みの
`refs.sqlite3` main fileで、最大512 MiBでなければならない。openerはsource SQLite connectionやread lockを
取得せず、source pathをSQLiteへ渡さない。main fileをprivate temporary fileへcopyしながらSHA-256を計算し、
copy後にsourceを再読して計算したSHA-256との一致を確認して、temporary copyだけをSQLiteでopenする。
`refs.sqlite3-wal`／`refs.sqlite3-shm`／`refs.sqlite3-journal`の存在、またはcopy中のsource変更によるdigest不一致は
`read_only_source_busy`でfail closedし、512 MiB超過は拒否する。SQLiteがread用sidecarを
必要とする場合もtemporary copyの隣だけに作り、source repositoryには作らない。destinationはsource
repository自身またはその子directoryにできず、parentは既存のreal directory、destination自身は不存在で
なければならない。生成fileを同じparent内のstaging directoryへ書込み・syncした後、対応platformでは
atomic no-replaceで公開する。

- CLIが発見するcomplete／incomplete creator sessionは合計最大100件で、超過は拒否する。
  `--session <id>`を指定しても、このrepository-wide discovery boundは緩和しない。
- `--session <id>`は一sessionだけを選ぶ。省略時はboundedに発見したcomplete sessionを対象にし、
  incomplete sessionはcomplete storyへ昇格させず状態だけを記録する。
- target省略時は`--target synapse`。`--synapse`はそのaliasである。
- `--github`は`--target github`のaliasで、localの`target/README.md`等を作るだけである。
  GitHubへのupload、Git command、API call、network requestは行わない。
- target selectorは相互排他で、一つでも重複すれば`usage_error`になる。
- `--public`はbundleのvisibilityを`public`として明示選択するだけで、外部へcopyしない。省略時は
  `private_review`で、manifestは外部copy前のreviewが必要であることを保持する。
- `--presentation`はauthor-suppliedなpublic-facing textを追加する。source historyの検証済み事実へ
  昇格せず、`projection.json`では`author_supplied`として区別する。

最小の`presentation.toml`例:

```toml
title = "North wall conservation history"
summary = "A public explanation of the recorded alternatives and Human decision."
creator_display_name = "Conservation team"
proposal_agent_display_name = "Caller-supplied AI-attributed proposal"

[sessions.wall-1]
title = "First review"
public_decision_note = "We retained this direction for a later material test."
original_caption = "First recorded state"
current_caption = "State at review"
proposal_caption = "Alternative retained in history"
```

sidecarは64 KiB以下のregular UTF-8 fileに限定し、symlinkとunknown fieldを拒否する。stored Human
rationale、internal Actor ID、repository path、raw asset bytesはsidecarや`--public`の有無にかかわらず
出力しない。`public_decision_note`はsource rationaleの公開化ではなく、別途authorが供給した公開文である。

bundle rootは次を含む。

```text
<output-dir>/
├── projection.json   canonical machine-readable semantics
├── story.md          escaped Human story
├── index.html        escaped, JavaScript-free static view
├── manifest.json     schema, target, visibility, source fingerprint
├── checksums.json    fixed inventory and SHA-256 digests
└── target/           target-specific layout; canonical semanticsを持たない
```

両targetのroot `projection.json`、`story.md`、`index.html`は同じ意味を持つ。`synapse` targetは
`target/public-projection.json`、`github` targetは`target/README.md`、`target/index.html`、
`target/projection.json`を追加する。raw imageやthumbnailはM0/M1 profileでは含めない。
machine-readableであることは学習許可を意味せず、training useは`prohibited`である。

成功時はdestination、target、visibility、projection digest、complete／incomplete session件数を
stdoutへ出す。`synapse-present --version`は`synapse-present <package-version>`を出力する。

### `preview <bundle-dir>`

既存bundleの固定inventory、checksum、schema、canonical `projection.json`、manifestとのsemantic link、
target-specific copyを検証する。成功時はtarget、visibility、projection digestとlocal `index.html` pathを
表示するだけで、browser起動、source repository access、外部通信は行わない。

```bash
synapse-present preview public-view
```

主なcompanion error codeは`usage_error`、`destination_exists`、`unsafe_path`、
`read_only_source_busy`、`repository_error`、`creator_report_error`、`projection_invalid`、
`bundle_invalid`、`storage_error`である。

## Repository layout

`init` または任意の repository command は、指定 root の下に次を作る。

```text
<repo>/
├── cas/
│   ├── objects/    immutable objects grouped by family and digest prefix
│   └── tmp/        publication staging files
└── refs.sqlite3    mutable Refs and append-only reflog
```

layout は local implementation detail である。`cas/objects` や SQLite を直接編集せず、Core API / CLI を使う。
ProjectionStoreはrepository layoutの正本ではなく、callerが別path／connectionへ明示的に構築する
disposable derived indexである。

## Commands

### `init <repo>`

repository directory、filesystem ObjectStore、SQLite RefStore を作成または開く。

```text
initialized <repo>
```

既存 repository の data は消去しない。

### `put-blob <repo> <file> [--claimed <oid>]`

file の raw bytes を streaming ingest し、Blob OID を出力する。

```bash
synapse put-blob .synapse image.png
synapse put-blob .synapse image.png \
  --claimed blob:sg-oid-v1:sha256:<64-lowercase-hex>
```

`--claimed` がある場合は再計算 OID と exact match しなければならない。
media type、filename、EXIF、access policy は Blob OID に追加されない。

### `put-record <repo> <file> [--claimed <oid>]`

JSON を strict parse、concrete Record schema、local semantic rule で検証し、canonical Record を保存する。
body の `object_type` は `record` でなければならない。

### `build-tree <repo> <file> [--claimed <oid>]`

既成 JSON body を ManifestTree family として validate + put する。

> この command は directory を走査して Tree JSON を自動生成しない。

### `commit <repo> <file> [--claimed <oid>]`

既成 JSON body を Commit family として validate + put する。

> この command は parent、author、timestamp、snapshot を自動生成しない。

### `put-object <repo> <file> [--claimed <oid>]`

body の `object_type` に従い、Record、ManifestTree、Commit を自動 dispatch する。
Blob は JSON ではないため `put-blob` を使う。

全 put command は成功時に OID だけを stdout へ出す。同じ object の再投入は同じ OID を返すが、
CLI は `Created` / `AlreadyPresent` の disposition を表示しない。

### `update-ref <repo> <ref> <expected-oid|-> <new-oid> [options]`

candidate Commit の typed closure を確認し、Ref と reflog を compare-and-swap 更新する。

```bash
synapse update-ref .synapse proposal/agent/run-1 - \"$NEW_HEAD\" \
  --actor actor:local-user \
  --message \"first proposal\"

synapse update-ref .synapse proposal/agent/run-1 \"$OLD_HEAD\" \"$NEW_HEAD\"
```

- `-` は current Ref が存在しないことを期待する create operation。
- update は current head の exact Commit OID を expected value にする。
- `--actor` と `--message` は各一回、順不同で指定できる。
- 成功時は `<ref><TAB><new-oid>` を stdout へ出す。
- closure failure または `ref_conflict` では Ref と reflog を変更しない。

allowed top-level namespace:

- `proposal/*`
- `decision/*`
- `release/*`
- `observed/*`
- `material-events/*`

これはRef nameのsyntax validationでありauthorizationではない。`--actor`の本人性も検証しない。
`update-ref`はlocal trusted operator向けの低水準`Repository::update_ref` primitiveであり、
Actor／Grant／Policy／runtime capability、project binding、Human Gate、ContextPack baseを検査しない。
AI用のcheckpoint／single-base-parent、snapshot output delta、transaction-time Grant expiryも強制しない。
AI callerには公開せず、`synapse-application`のone-shot routeを使用する。applicationは認証後のexact
project map／ACL、trusted profile／one-time current-head registration、Core `preflight_proposal`、opaque permit、
trusted Executor、実行後reauth／FIFO fenceを経て`publish_preflighted`を呼ぶ。
一般的なHuman Decision publicationもlow-level CLIではなくapplicationを使用する。成功したAI receiptのsame-instance admitted handle、
reusable Human profile、server-fixed candidateからone-time registration／permitを作り、authentication／exact
project ACL／FIFO fenceを通して`HumanDecisionRuntime::publish_decision`を呼ぶ。handleはCore denial後の修正版へ
再利用できるが、registrationとpermitはone-shotである。以下の`creator-run`はこのrouteをfixed local Pilotとして
内部利用する限定commandである。

### `creator-run <repo> <session> <original> <current> <ai-output> --subject <label> --creator <name> --decision <adopt|reject|defer> [--rationale <text>]`

手書きJSONなしで、一つのlocal single-creator sessionをcreateする。

```bash
synapse creator-run .synapse-creator mural-1 \
  original.png current.png ai-output.png \
  --subject "North wall mural" \
  --creator "Aki" \
  --decision adopt \
  --rationale "The proposal fits the intended palette."
```

- repositoryは開くか新規作成する。
- `session`は`[a-z][a-z0-9-]{0,63}`。session固有の
  `proposal/creator-agent/<session>`と`decision/creator/<session>`がどちらも存在しない場合だけcreateする。
  両Refがありdecision headがcompleteなDecision Commitなら再実行は`creator_session_exists`、片方だけ、または
  decision Refがまだbase Commitを指すpartial stateなら`creator_session_incomplete`で、既存履歴を上書きしない。
- `original`、`current`、`ai-output`はraw bytesのopaque Blobとしてstreaming ingestする。original／currentという
  roleはcaller-suppliedである。originalとcurrentのprimary Blob OIDは後述のbyte-identity adapterで比較するが、
  media type、EXIF、pixel、画像内容をdecodeせず、画像registrationやvisual／physical changeを判定しない。
- `ai-output`はtrusted local integrationが事前に用意したfileである。commandはmodel、connector、paint toolを
  起動せず、prepared local ExecutorがそのOIDをAI proposal outputとして返す。
- `--subject`は1〜500 UTF-8 bytes、`--creator`は1〜300 UTF-8 bytes。`--creator`はself-declaredな表示名で、
  OS userや実利用者credentialの本人性を証明しない。
- 各runのcreator、agent、project、Subject等のEntityIdはOSの暗号学的乱数から新規生成するsession-local IDである。
  Subject extension `org.synapsegit.creator-session`のmanifestへ保存し、report／archive restoreで復元するが、
  同じ`--creator`をsession横断で識別するglobal ID、credential、identity registryではない。
- 両Observationは同じCaptureProfileを参照する。初版は`profile_level=imported`、
  `allowed_claims=[reference_only]`、verified required conditionなしであり、repeatable／calibrated captureを主張しない。
- `--rationale`は任意で最大5,000 UTF-8 bytes。省略時はdecision別の既定rationaleを記録する。
  DecisionFeedbackの既定は`reason_codes=["unspecified"]`、`visibility=private`、
  `training_use_policy=prohibited`である。
- fileに外部検証済み時刻がないため、生成するObservationの`capture_time`とActivityの`valid_time`は
  `unknown`である。各stageのRecordにはrun内でstrictly monotonicになるrecording timestampを保存するが、
  `recorded_at`を撮影・生成・実行時刻や外部eventの物理順序の証拠として扱わない。
- Subject、human／AI／comparison software-tool Actor、Policy、DelegationGrant、imported CaptureProfile、
  original／current Observation、byte-identity AnalysisResult、import Activity、ContextPack、AI Activity、
  base／proposal／decision Commit、DecisionFeedback、ManifestTreeを自動生成する。comparison toolはAI Actorとは別Entityで、
  adapter implementation／configuration Blobとorderedなbase／target Observation lineageもbase snapshotへ保存する。
- base session bootstrapはtrusted local orchestrationである。proposalは`Application`のAI preflight／one-shot
  permit／full Core admissionを、decisionは同じinstanceのadmitted proposal handleとHuman one-shot routeを通る。
- `adopt`はprotocolの`adopted_unchanged`でproposal snapshotを選ぶ。`reject`／`defer`はそれぞれ
  `rejected`／`deferred`としてbase snapshotを維持する。どの場合もproposal Ref、AI output、AI Activityを残す。
- completion前にrepository全体を`fsck`し、issueがあれば`fsck_failed`にする。base Ref publication前のfailureでは
  immutable orphanが残り得る。base Ref publication後かつHuman Decision前のfailureはliveなincomplete sessionを
  残し、次回runは`creator_session_incomplete`になる。Decision publication後のfailureはcomplete sessionを
  残し得る。create-only Pilotはどちらも自動resume／cleanupやexisting Refの上書きを行わない。
- archive export／restoreは自動実行しない。完了後に通常の`export`／`restore` commandを使う。

成功時はsession receiptを出力した後、同じsessionの`creator-report`も続けて出力する。receipt部分は次の形式である。

```text
session=<session>
subject=<subject-entity-id>
original=<blob-oid>
current=<blob-oid>
ai_output=<blob-oid>
proposal_ref=<ref><TAB><commit-oid>
decision_ref=<ref><TAB><commit-oid>
disposition=<adopt|reject|defer>
```

AI／Human publicationとcompletion `fsck`は、続けて表示するreportの構築より先に完了する。commit済みsessionの
report再構築だけが失敗した場合、`creator-run`は`creator_report_unavailable_after_commit`を返す。このerrorは
rollbackやincomplete sessionを意味しない。receipt／reportはまだstdoutへ出していない場合があるが、同じsessionを
`creator-run`で再実行せず、messageどおり`creator-report <repo> <session>`で状態と原因を再確認する。

このcommandのlocal Authenticator、project ACL、profiles、credentials、Executor、permitは一process内の
fixed Pilot stateである。HTTP／JWT、durable／distributed ACL、OS sandbox／egress、external model execution、
multi-user／organization／quorum／release、modified／partial adoptionを提供しない。

### `creator-report <repo> <session>`

current `proposal/creator-agent/<session>`と`decision/creator/<session>`からcreator sessionを再構築する。

```bash
synapse creator-report .synapse-creator mural-1
```

reportは一つのconsistent `RefSnapshot`を取得し、そこからcurrent decision／proposal headを両方解決する。
decisionのbase Tree内にあるSubject extension manifestからsession-local EntityIdを復元し、current decisionが
single base parent／single DecisionFeedbackを持つこと、current proposalがそのbaseをparentにしてsingle AI Activityを
持つこと、author／entity／responsible principal／proposal binding、dispositionとdecision snapshot（adoptならproposal、
reject／deferならbase）の対応を再検証する。同じ`RefSnapshot`からin-memory `SqliteProjectionStore`をrebuildし、
sessionのdecision／proposal RefだけにscopeしたSubject timelineとoriginal／current／AI output OIDを得る。
timelineのAI Activityはcurrent proposal transitionと一致しなければならない。新しいcreator sessionでは、同じscopeの
Analysis lineageからordered input、adapter digest、software-tool attribution、両creator Refからの到達性、
prerequisite object availabilityも検証する。
最後にrepository全体を`fsck`し、lineage不一致は`creator_report_invalid`、integrity issueは`fsck_failed`で拒否する。
Projectionは一時的なderived query stateで、authorization、archive、recoveryの入力ではない。

主要出力は次の形式である。`selected=true`は`adopt`だけで、`reject`／`defer`ではfalseになる。
`proposal_attributed_to_agent`はPilotが記録したattributionであり、commandやmodelによる生成証明ではない。
`ai_output_source=caller_supplied`が第三fileの入力由来を明示する。`reviewed_by_human`はreviewerを表し、
proposalを選んだという意味ではない。libraryの`CreatorReport`は`selected_ai_output`と
base／proposal／decision snapshotを返し、text CLIは前者を`selected`として表示する。

```text
report_session=<session>
project=<project-entity-id>
subject=<subject-entity-id>
proposal_attributed_to_agent=<agent-entity-id>
ai_output_source=caller_supplied
reviewed_by_human=<creator-entity-id>
selected=<true|false>
base_head=<commit-oid>
base_snapshot=<tree-oid>
proposal_snapshot=<tree-oid>
decision_snapshot=<tree-oid>
decision_ref=<ref><TAB><commit-oid>
proposal_ref=<ref><TAB><commit-oid>
disposition=<adopt|reject|defer>
rationale=<quoted-text>                 # present when stored
original=<blob-oid>
current=<blob-oid>
ai_output=<blob-oid>
comparison_analysis=<analysis-result-oid>
comparison_adapter=synapsegit.observation.byte-identity@1
comparison_status=succeeded
comparison_comparability=partial
byte_identity=<identical|different>
comparison_reason_codes=byte_identity_only,capture_profile_imported,capture_time_unknown
comparison_replay_ready=true
comparison_warning="<conservative-interpretation-warning>"
fsck=clean objects=<count>
timeline=<count>
<ordering-time><TAB><time-basis><TAB><stage><TAB><observation|activity><TAB><entity-id><TAB><record-oid><TAB><reachable-ref-list>
```

`byte_identity=identical`は二つのverified primary Blob OIDが同じ、`different`は異なる、というbyte-levelの結果だけである。
同じbytesでも観測した物理対象が不変とは限らず、異なるbytesでもvisual／physical changeを確定できない。
adapterはpixel／EXIFをdecodeせず、viewpoint registration、外観差分、色・寸法比較を行わない。このため成功時も
`comparison_comparability=partial`とする。`comparison_warning`は`identical`なら
`Identical Blob bytes do not establish that the observed physical subject was unchanged.`、`different`なら
`Different Blob bytes do not establish visual or physical change.`をquoted textとして表示する。
`comparison_replay_ready=true`はinput Observation、adapter implementation／configuration等のprerequisite objectが
Projectionから利用可能と確認できたことだけを表す。adapterを実行できるruntime、environment、determinism、
byte-identical／semanticなexact replayを保証しない。

base Treeにcomparison entryを一つも持たないlegacy-shaped creator sessionは、他のlineage検証を通過した場合に
`comparison=unavailable`を表示する。このshapeはsessionの作成時期を証明せず、`identical`、`different`、
「変化なし」のいずれも意味しない。comparison entryが一部だけあるsessionはlegacy扱いにせずinvalidとして拒否する。

両creator Refがない場合は`creator_session_not_found`。`export`したarchiveをempty repositoryへ`restore`した後も、
同じsessionのreportとOIDを再構築できる。reportのordering timeはRecordに保存されたordering basisであり、
このPilotでは`original_observation`、`current_observation`、`image_import`、`ai_proposal` stageの単調増加する
recording timestampになる。time basisはObservationなら`observation_recorded_at_fallback`、Activityなら
`activity_recorded_at_fallback`であり、unknownなcapture／valid timeを撮影時刻、AI execution time、
外部eventの物理順序へ昇格させない。

### `refs <repo>`

current Refs を name 順に出力する。

```text
<name><TAB><commit-oid>
```

reflog を表示する CLI command はまだない。

### `fsck <repo>`

stored object の pathname / OID / canonical bytes と、current Ref heads からの closure を検査する。

```text
objects=<seen> verified=<verified> closures=<roots> issues=<count>
```

issue detail は stderr に Rust Debug 形式で出し、問題があれば最後に `fsck_failed` で終了する。
Ref が0件なら stored Commit 全件を root とする。

`fsck` は全 structured object の concrete JSON Schema を再実行しない。
Core ingest / restore を通った object は投入時に schema 検証済みである。

### `export <repo> <archive-dir>`

Refs / reflog を snapshot し、ObjectStore 全体を checksum 付き directory archive へ export する。

- destination は存在してはならない。
- reachable object だけでなく、stored orphan object も含む。
- current Ref heads または historical reflog `new_head` の closure が不完全なら拒否する。
- 全headで一つのbounded Tombstone catalogを共有する。既定はRecord 100,000件／累積1 GiB。
- distinct headのclosure validationは`max_head_validation_nodes=1,000,000`／
  `max_head_validation_edges=10,000,000`が既定。異なるheadが共有closureを再走査したworkも再課金し、
  各headには残量とRepository `GraphLimits`の小さい方を使う。
- complete CAS inventoryは既定100,000 object、copied raw object bytesは累積1 TiBまで。
- Ref snapshot／complete reflogは各100,000 entries、保持するRef名／actor／messageは累積64 MiBまで。
- 生成manifestがrestore側と同じ64 MiB上限を超える場合も`resource_limit`で拒否する。
- current CLIはlibraryのarchive export既定値を使い、override optionは提供しない。library callerは
  `ArchiveExportLimits`でobject／byte／head validation work／scan／Ref snapshotのdefaultを小さくも
  大きくも置き換えられる。0または超過は`resource_limit`になる。
- resource limit失敗ではfinal destinationを公開せず、通常のerror returnではstagingを除去する。
- destinationと同じparentにprocess IDと時刻nonceを含むper-export stagingを使う。process crashでは
  orphan stagingが残り得て、startup cleanupはまだない。
- final publicationはLinux、Android、Apple、Redoxでatomic no-replaceを使う。その他target
  （Windowsを含む）は`storage_error`でfail closedする。
- archive は暗号化・署名されない。
- 成功時は `exported <archive-dir>` を出力する。

format detail は [Local directory archive profile](../spec/core/v0.1/archive-profile.md) を参照する。

### `restore <archive-dir> <repo>`

directory archive を検証し、object、reflog、Refs を復元する。

- Ref / reflog が既に存在する repository は拒否する。
- CAS は空、または同じ manifest OID 集合の exact subset なら失敗 restore を再開できる。
- unrelated object が一つでもあれば `archive_not_empty`。
- object、checksum、OID、schema、closure を再検証する。
- Ref / reflog は object phase と closure 検証の後に公開する。
- 成功時は `restored <repo>` を出力する。

## Ref name constraints

- 全体は最大500 bytes。
- slash 区切りの各 segment は1〜128 bytes。
- segment は ASCII lowercase letter または digit で始める。
- 以降は ASCII lowercase letter、digit、`.`、`_`、`:`、`-`。
- empty segment、`.`、`..`、上記以外の top-level namespace は拒否する。

## 既定 limit

| resource | default |
|---|---:|
| CLI structured file read | 16 MiB |
| Blob | 512 MiB |
| creator session / subject label / creator name / rationale | 64 ASCII bytes / 500 / 300 / 5,000 UTF-8 bytes |
| JSON depth | 128 |
| JSON nodes | 100,000 |
| container members / items | 50,000 |
| closure objects / edges / depth | 100,000 / 1,000,000 / 512 |
| closure dynamic reference-role metadata | 64 MiB（hard ceiling） |
| archive distinct-head validation nodes / edges | 1,000,000 / 10,000,000 |

完全な limit と security boundary は [Security model](./security_model.md) を参照する。

## Error code guide

| code | 意味 |
|---|---|
| `usage_error` | CLI argument または CLI-side structured size error |
| `storage_error` | filesystem、SQLite、clock、unsupported local storage state |
| `invalid_utf8` / `bom_forbidden` | structured input encoding rejection |
| `duplicate_key` / `number_token_forbidden` / `unsafe_integer` / `lone_surrogate` | strict JSON rejection |
| `key_not_nfc` / `identifier_not_nfc` | canonical identity rule rejection |
| `set_not_sorted` / `set_duplicate` | schema-marked set rejection |
| `timestamp_invalid` / `interval_invalid` | canonical time rejection |
| `fixed_point_not_normalized` | ScaledInteger normalization rejection |
| `path_segment_invalid` | Manifest path または Ref name rejection |
| `schema_invalid` | concrete schema / local semantic / graph shape rejection |
| `reference_type_mismatch` | object family または known Record target type mismatch |
| `oid_mismatch` | claimed OID、stored byte、archive object の不一致 |
| `closure_missing` | required object が unresolved |
| `authorization_denied` | admission libraryでidentity／binding／capability／Policy／snapshot／disposition／duplicate等を拒否。`creator-run`はAI／Human routeから透過し得る |
| `human_gate_required` | AIのdecision/release直接更新、または現在のtrusted routeが満たさないPolicy gate |
| `stale_base` | Creative AI publicationでContextPack expected baseとlive base Refが不一致。Human Decisionのdecision/base競合は`ref_conflict` |
| `ref_conflict` | current target Ref、またはHuman Decisionのtrusted proposal Ref/headとexpectationが不一致 |
| `resource_limit` | Core parser / graph / Blob、graph dynamic reference metadata、archive inventory / bytes / head-validation work / manifest limit |
| `archive_invalid` | export destination exists、export元head closure invalid、またはrestore manifest／checksum／archive graph invalid |
| `archive_not_empty` | restore target に unrelated data または existing Ref がある |
| `fsck_failed` | `fsck`、`creator-run`、`creator-report`が一件以上のintegrity issueを発見 |
| `creator_session_exists` | `creator-run`のproposal／decision Refがcomplete sessionとして既に存在するcreate-only conflict |
| `creator_session_incomplete` | base Ref公開後等にproposal／decision Refがpartial stateで残ったcreate-only session。自動resume／cleanupしない |
| `creator_session_not_found` | `creator-report`に必要なcurrent proposal／decision Refがない |
| `creator_report_unavailable_after_commit` | `creator-run`のsession commit／fsckは完了したが、続くreport構築が失敗。再実行で上書きせず`creator-report`で再確認する |
| `creator_report_invalid` | current proposal／Feedback／decision snapshot／AI Activity、またはcomparison Tree set／Analysis／tool Actor／adapter digest／ordered input／replay prerequisite／両Ref reachabilityがcreator contractと一致しない |
| `authentication_required` | application routeのcredentialをAuthenticatorが受理しない。`creator-run`のfixed Pilot routeから透過し得る |
| `project_access_denied` | application routeのmalformed／unknown／forbidden project、またはproject-scoped handle/profile不一致。`creator-run`から透過し得る |
| `execution_permit_invalid` | AIまたはHuman application permitがwrong session／instance、consumed、revoked、expired、またはClock backward。`creator-run`から透過し得る |
| `execution_failed` | trusted application Executorがerrorまたはpanic。`creator-run`から透過し得る |
| `configuration_invalid` | trusted application control-plane configurationが不正。`creator-run`から透過し得る |
| `service_unavailable` | applicationのlock／counter等のoperational failure。`creator-run`から透過し得る |

上表のCore admission codeは`CreativeAiRuntime`／`HumanDecisionRuntime` library boundaryで実装されている。
現在のCLI `update-ref`はContextPackやtrusted identity／Policy／proposal authorityを受け取らないため、
admission authorizationや`stale_base`を評価せず、target Ref競合には`ref_conflict`を返す。
これらapplication codeのexact messageは順に`authentication required`、`project access denied`、
`execution permit invalid`、`execution failed`、`application configuration invalid`、
`application service unavailable`である。Core semantic errorはauthorized AI／Human permitをburnしたfinal
publicationからだけ透過される。AI preflight denialは`configuration_invalid`、そのoperational failureは
`service_unavailable`へ正規化され、Humanに別Core preflightはない。low-level commandはこれらを返さないが、
AI／Human routeを内部利用する`creator-run`は返し得る。

## 未実装 command

`read-object`、`log`、`diff`、`checkout`、`merge`、Ref delete、object delete / GC、reflog view、
JSON output、stdin input、`publish-proposal`、`publish-decision`、projection `rebuild`／timeline／
dependency／Analysis lineage／closureのgeneral queryは現在の CLI にない。`creator-report`はcreator session専用の
bounded timelineである。ProjectionのRust APIは実装済みだが、automatic refresh、
SurrealDB adapter、全8-query／benchmark比較は未実装である。
`publish-proposal`／narrow `publish-decision`相当のprocess-local application library routeが実装されたことは、
HTTP／JWT、durable permit service、一般的なHuman workflow、Projection application route、または
general Human Decision／Projection CLI commandを提供したという意味ではない。`creator-run`は3画像・single creator・
create-only sessionへ固定したPilot orchestrationである。
