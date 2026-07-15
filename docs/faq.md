# SynapseGit FAQ

## SynapseGit は Git の fork ですか

違う。Git の content-addressed object、Commit DAG、Ref compare-and-swap、reflog、fsck という原則を借りるが、
Git repository format や source-code workflow を拡張した fork ではない。
物理・デジタル創作の Evidence、Observation、Claim、Decision を別の Core protocol で表す。

## 既存の Git、BIM、CAD、ペイント tool を置き換えますか

置き換えない。既存 tool の Artifact と、物理 Subject、観測条件、判断理由を横断して接続する layer を目指す。
現在はlocal Core CLI、process-local authenticated AI + narrow Human Decision application route、両Core admissionに加え、
3 fileから履歴を構成するcreate-only creator Pilotと、original／current Blobのbyte identityだけを記録する
保守的なObservation adapterをRustで実装している。既存制作toolとの常時連携やGUIを置き換えるものではない。

## 今すぐ何を動かせますか

strict JSON / OID、15 Record type の schema、filesystem ObjectStore、Commit closure、Tombstone availability、
SQLite Ref CAS / reflog、`fsck`、directory export / restoreを動かせる。Rust APIではさらに
`synapse-application`によるlocal authenticated one-shot AI executionとsame-instance admitted proposalに
限定したHuman Decision route、`CreativeAiRuntime`によるAI proposal preflight／publication admission、
`HumanDecisionRuntime`によるtrusted single-humanのnarrow `decision/*` admissionを利用できる。CLIの
`creator-run`／`creator-report`では、imported CaptureProfile、2 Observation、専用software-tool Actor、
byte-identity AnalysisResult、AI proposal、Human Decisionを手書きJSONなしで作り、archive restore後も
同じreportを再構築できる。
[Quickstart](./quickstart.md) は同梱 fixture だけで end-to-end 実行できる。

byte-identity adapterはverified primary Blob OIDだけを比較し、比較成功時も`partial`／`byte_identity_only`として
扱う。subject／series不一致やprimaryの欠落・曖昧さは`not_run`／`incomparable`である。pixelやEXIFをdecodeせず、
同一bytesから物理的不変を、異なるbytesから視覚・物理変化を推論しない。
capture client、pixel-level画像registration / diff、HTTP／JWT、durable／distributed ACL・permit、Projection
application route、release／modified／partial／quorum workflow、ExecutorのOS sandbox／egress、
Grant revocation、SurrealDB adapterはまだない。SQLite projection baselineはlibraryとして
実装済みだが、projection CLI、自動refresh、全8-query／benchmark比較はない。二つのadmissionもCLI commandとして未公開である。

## hash で作者や真実を証明できますか

できない。OID が確認するのは profile に従った byte identity である。
作者性、撮影時刻、現実の状態、著作権、permission、契約適合は別の Evidence、Claim、Assurance、Policy と review が必要である。

## `sg-oid-v1` は freeze 済みですか

まだ Stage 0 draft である。Rust と JavaScript fixture は一致しているが、
第二の独立 production implementation が schema / semantics まで通す freeze gate は未達である。

## AI は本当に `proposal/*` しか変更できませんか

`synapse-application` routeは認証後にexact project map／ACLからserver-owned authorityとtargetを選び、
`synapse-core::CreativeAiRuntime`を通るAI publicationではproposal-onlyを強制する。reusable profileが
actor、project、principal、human-gated base Ref、authority snapshot、ContextPack、pre-authorized exact
capability set、runtime capability、target Ref名／side effectを固定し、one-time registrationが
profile generationとtargetのexact current-head expectationをsealする。
Actor、AI Activity、ContextPack、DelegationGrant、Policy、candidate Commitをcross-checkし、AIは許可された
`proposal/*`だけを書け、`decision/*`／`release/*`は`human_gate_required`で拒否される。

ただし現在のCLIはこのrouteを公開しない。CLIの`update-ref`と`Repository::update_ref`はlocal
trusted operator向けの低水準primitiveで、全allowed namespaceを更新できる。`--actor`は本人認証ではなく
reflog metadataにすぎないため、untrusted AI callerへこの低水準routeを渡してはならない。

## authenticated AI execution route は何を保証しますか

request planeはcredential、project selector、opaque `RegisteredExecutionHandle`／`AiExecutionPermit`だけである。
injected `Authenticator`をproject lookupより先に呼び、caller文字列をrepository pathへjoinせずexact mapと
process ACLを使う。malformed／unknown／forbidden projectは同じ`project_access_denied` responseである。

applicationはcandidateなしのCore preflightからopaque non-Clone permitを作り、exclusive TTL内でpermitを
使用する。credential rejectionはpermit lookupより先に`authentication_required`となり、ready permitを消費しない。
authentication成功後にmatching permit registry entryをclaimした時点で再利用不能にburnし、それからsingle trusted
`AiExecutor`を起動する。実行後は再認証してからproject FIFO fenceへ
入り、live ACL／profileを再確認し、full Core revalidation／CASまで保持する。preflightはRef予約ではなく、
Executor failure、expiry、revocation、Core denial、conflict後もpermitは戻らない。

これはprocess-local Rust libraryである。HTTP／JWT、restartを越えるACL／permit、multi-process ordering、
OS sandbox、connector／egress control、Projection routeは保証しない。Human routeもこの同じprocess-local
boundaryに限定される。

## authenticated Human Decision route は何を保証しますか

AI publication成功時の`AiPublicationReceipt`はCore authorization decisionと、application
instance／project／proposal Ref/headへ束縛されたopaque non-Clone `AdmittedProposalHandle`を返す。
Human control planeはdirect human／canonical decision Ref／Human Actor／Policyを固定するreusable profileと、
new Decision Commit／DecisionFeedback／messageだけのserver-owned candidateを用意し、そのhandleをborrowして
one-time registrationを作る。requestはcredential、exact project selector、opaque registration／permitだけを渡し、
human、proposal、candidate、OID、Ref expectationを選べない。

prepareは認証をlookupより先に行い、exact process ACL、live profile generation、registrationを同じproject
FIFO fenceで検査して、application TTL (`now < not_after`) だけのpermitを発行する。publishは認証失敗なら
ready permitを残し、認証後にmatching permitをclaimした時点でburnする。その後、TTL／backward Clock、live
ACL／profileを再検査し、fenceを`HumanDecisionRuntime`のfull immutable validation、proposal Ref
precondition、canonical decision Ref CASまで保持する。Human routeに別ExecutorやCore preflightはない。
invalid Human permitも`execution_permit_invalid`であり、Core errorはburn後のfinal publicationだけから透過する。
publish認証は冒頭の一回だけで、fence／state lock／Repository lock内からAuthenticatorを再実行しない。
認証resultはpoint-in-timeなsession decisionである。このfenceはprocess-local ACL／profile mutationを
線形化するが、queued requestに対する外部credential storeの即時revocation fencingは保証しない。
permit TTLがwindowをboundedにし、production auth adapter／credential lease semanticsはdeployment責任である。

admitted handle自体はprocess-local evidenceとして再利用できる。candidate／Core denial後に修正版を再登録できるが、
各registrationとpermitはone-shotである。一proposalのcanonical dispositionはhandleの消費ではなく、Coreの
duplicate lineage検査とdecision Ref CASで一つに保つ。このhandleはportable receipt／signatureでもrestart後の
proofでもなく、別application instanceやprojectへ移せない。

## AI candidateとoutputにはどんな制限がありますか

Stage 0 candidateは`commit_kind=checkpoint`で、唯一のparentが`ContextPack.base_commit`でなければならない。
既存proposalを更新してもcurrent proposalをparentにせず、mergeやproposal chainは後続scopeである。
baseのcurrent snapshotにagent／principal Actor、Grant、Policyのexact OIDが必要で、ancestorだけの存在では不足する。
candidate snapshotはさらにbase snapshotの全non-Tree objectを保持する。Tree OIDは置換／再配置できるため、
path-levelの不変性を要求するruleではない。

candidate／base snapshot差分では、admission用Activity／固定ContextPackを除く新規non-TreeをActivity outputへ
束縛する。generated output closureはexplicit output closureからContextPack selected input closureを引き、
explicit output rootだけを戻す。したがってinput-only dependencyのbytes／assertion／typeはoutputとして
二重評価しないが、Tree-only residualを除きbase snapshot外のselected inputをcandidate snapshotへ置くなら
Activity outputとして明示する必要がある。output Recordはagent自身がassertしたAnalysisResult／Claimだけで、
Tombstone、control Record、nested Commitは拒否される。quotaはgenerated output closureと新規TreeをOID重複なしで数える。

current Activityが生成するClaimは`payload.ai_run_ref`を省略する。provenanceの正本はActivity
`output_refs`からClaimへのedgeである。同じActivityへの逆参照はcontent-addressed OID cycleになり、
旧runへの参照も今回の生成元を誤表示するため、Stage 0 admissionはどちらも拒否する。
新しいTreeだけでbaseのnon-Tree objectを保持しながら再配置するproposalは許可されるが、
decision／releaseへの採用はAI routeでは行えない。

## Human Decision runtime は何を承認できますか

上位serviceが認証・project access確認済みとして渡したdirect single humanについて、
`HumanDecisionAuthority`がHuman Actor／Policy、canonical decision Refとexact current head、exact proposal Ref/head、
AI Activity／ContextPack／Grant／base chainを固定する。`publish_decision`はPolicy `publish`を評価し、
`before_decision_ref`だけを満たしたうえでDecision CommitとDecisionFeedbackを検査する。
reviewerはAI responsible principal、ContextPack／Grant asserter、Grant direct principalと同一でなければならず、
PolicyはContextPackのexact snapshotである。proposal transitionはexactly one AI Activity、decision transitionは
exactly one DecisionFeedbackに限定される。

- `adopted_unchanged`: proposal snapshotを採用する。
- `rejected`／`deferred`／`experiment_only`: base snapshotを保持して判断だけを記録する。
- `adopted_modified`／`partially_adopted`: human modification provenanceが未定義なので拒否する。

同じproposalへのDecisionFeedbackがcanonical decision lineageにあれば再決定を拒否する。
Context baseとproposal Refはdecision CAS／reflogと同じtransactionで検査される。上位のinitial application
routeはinjected Authenticatorとprocess ACLを実行するが、runtime自身はcredential verifierではない。
concrete credential／persistent membership、organization代理、quorum／MFA、release approvalは未実装である。

## AI authorizationでまだ実装されていないものは何ですか

concrete HTTP／JWT／MFA、credential database／rotation、durable／distributed ACL・permit、multi-process
publication fence、Projection authenticated route、organization／quorum、release approval、
modified／partial adoption、multi-project CAS membership／classification resolver、model processの
sandbox／connector／egress／physical-effect enforcement、Grant revocationである。initial AI application routeは
Executorをpermit後へ順序付け、narrow Human routeはsame-instance admitted proposalだけを扱うが、trusted
Executorの動作をOS／networkで隔離する境界でも、一般的なHuman workflowでもない。

## SQLite と SurrealDB は何に使いますか

`synapse-sqlite`はmutable Refとreflogをtransaction管理する。filesystem ObjectStoreが
immutable objectのsource of truthである。

別crateの`SqliteProjectionStore`は、一つのcaller-supplied Ref snapshotとverified ObjectStoreから
current reachable objectだけをexplicit・atomic rebuildするquery indexとして実装済みである。
orphanを除外し、schema version／source fingerprint、missing closure issue、tombstoned availability／count、
Ref-scoped Subject timeline、Observation dependencies、typed AnalysisResult lineageを保持する。Analysisの
replay `Ready`はinput／adapter／configuration／transformがpresentというだけで、derived output／maskは
blockせずexact replayも保証しない。自動更新されないため、freshnessはcallerが管理する。
rebuild中はarchive exportと同じcooperative append-only／no concurrent removalを前提とし、途中で
present objectが消えた場合はmissingへ格下げせず失敗して旧projectionを維持する。serviceは失敗と
fingerprint／freshnessを監視するが、projectionの古さを認可へ使わない。

`RefScope`はACLやtenant isolationではなくquery filterである。Analysis queryは「globalに未index」と
「index済みだがselected Refから未到達」を区別するため、未認可callerへ返すと存在oracleになり得る。
serviceはauthoritativeなproject／Ref accessを先に検査し、認可後だけresult／errorを公開する。

これはauthorization、ObjectStore、RefStore、archive input、recovery prerequisiteではない。SurrealDB adapterを追加し、
8 representative query、performance、migration、rebuild costで両backendを比較する作業は未実装である。
SurrealDBもOID／archiveの正本にはしない。

## Python や TypeScript が OID を決めてもよいですか

Stage 0 では Rust Core が canonicalize / validate / OID calculation の authority を持つ。
Python image / AI adapter と TypeScript UI は input body と Blob を提出し、Core が返す OID を使う。
独立実装は conformance fixture を通して相互検証する。

## Tombstone を作れば全 copy が消えますか

消えない。Tombstone は target payload が利用不能になった履歴を解決する Record である。
現在の CLI に delete / key destruction / derived purge command はない。
既に export、backup、third party へ渡した copy は Core から回収できない。

## archive は一つの file ですか

現在は非圧縮 directory である。

```text
archive/
├── manifest.json
├── manifest.sha256
└── objects/
```

layout と validation rule は [archive profile](../spec/core/v0.1/archive-profile.md) を参照する。
checksum は整合性検査で、暗号化や署名ではない。

## 既存 repository へ restore できますか

Refs / reflog が空で、CAS が空、または同じ archive の失敗 restore が残した exact object subset の場合だけ可能。
unrelated object や既存 Ref があれば拒否する。
merge / import operation ではなく、同じ repository state の復元である。

## raw image や EXIF は private ですか

Core protocol は private raw + redacted preview を推奨するが、current local CLI は encryption / ACL を実装しない。
put した Blob と export archive は local filesystem permission の範囲で平文保存される。
production service は encryption、access control、EXIF / face / location redaction を別途実装する必要がある。

## `ref_conflict` と `stale_base` の違いは何ですか

- `ref_conflict` は実装済み。Ref update の expected head と current head が異なるため atomic update を拒否した状態。
- `stale_base` も`CreativeAiRuntime`で実装済み。ContextPackが束縛したexpected baseとlive base RefがずれたAI outputを示す。

AI routeはbase Ref preconditionとproposal target CAS／reflogを同じSQLite transactionで扱うため、
競合時にRefとreflogを変更しない。低水準CLI `update-ref`はContextPackを受け取らないため
`stale_base`を評価せず、target Refの`ref_conflict`だけを扱う。
Grantの時刻はtransaction前だけでなくSQLite `BEGIN IMMEDIATE`直後、Ref stateを読む前にも再検査するため、
writer lock待機中に`expires_at`へ達したpublicationやbackward clockはfail-closedになる。
error precedenceはRef lexical validation、namespace gate／proposal-only、candidate closure、残りの
authorization／初回expiry、transaction Clock guard、`stale_base`、target `ref_conflict`の順である。
decision/releaseはcandidateを読まず`human_gate_required`となり、proposal requestがunauthorizedかつ
staleならbase stateを開示せず`authorization_denied`を返す。

`HumanDecisionRuntime`でもauthorizationをlive state readより先に行い、ContextPack baseをtrustedな
canonical decision Ref/headへ一致させる。immutable chainの不一致は`authorization_denied`となる。
その後、trusted proposal Ref/headまたはlive canonical decision/base headが移動していれば
`ref_conflict`となり、Human routeは`stale_base`を返さない。どの失敗でもdecision Refとreflogは変化しない。

## `build-tree` と `commit` は JSON を自動生成しますか

しない。どちらも利用者が用意した JSON body を family 指定で validate + put する。
directory scan、author / timestamp / parent insertion、working tree checkout は未実装である。

## `fsck` が clean なら全 schema も再検証済みですか

そうとは限らない。`fsck` は全 stored object の pathname、OID、canonical bytes と選択した Commit closure を検査するが、
全 structured object に concrete JSON Schema を再適用しない。
production Core ingest / restore を通った object は保存前に schema 検証される。

## どの資料が最新仕様ですか

[documentation index](./README.md) の「資料の位置づけ」に従う。
形と identity は JSON Schema / OID profile、graph / Ref semantics は Operations、
現在の command と実装境界は Quickstart / CLI reference / Security model を確認する。
`init_plan.md` は historical source vision で、現行仕様ではない。
