# SynapseGit Core security and trust model

この文書は Stage 0 local implementation の trust boundary と、利用者が誤解してはいけない保証範囲をまとめる。
脅威モデルの完成版や production deployment guide ではない。

Status: **Stage 0 draft**<br>
Implemented scope: local object / Ref / archive integrity path, process-local authenticated Creative AI and narrow Human Decision routes, Core admissions, and disposable SQLite projection libraries<br>
Out of scope: concrete HTTP/JWT identity、durable/distributed authorization state、OS sandbox/egress、production tenant isolation

## 何を確認でき、何を確認できないか

| mechanism | 確認できること | 確認できないこと |
|---|---|---|
| content OID | 同じ profile で再計算した byte identity | 作者性、内容の真実、権利、許可 |
| schema / local semantics | Core v0.1 が受理する形と object 内の制約 | 現実世界の正しさ、参照先の存在 |
| Commit closure | snapshot から必要 object が解決可能か | 各 Claim が真実か |
| Ref CAS + reflog | stale update の拒絶と更新履歴 | actor の本人性。認証は別 layer |
| `CreativeAiRuntime` | trusted inputに対するAI proposal publicationの能力・object binding・namespace・base整合 | 人間の本人認証、AI実行前のegress/side effect防止、Human Gate承認そのもの |
| `synapse-application` initial routes | AIではexact project ACL、Core preflight、exclusive-TTL permit、trusted Executor、実行後reauthを、Humanではsame-instance admitted proposal、server-fixed candidate、one-shot permitを束縛し、両方をlive profile／FIFO fenceからCore full validationへ接続 | Authenticator実装自体の強度、HTTP/JWT、restartを越えるACL／permit、multi-process ordering、OS sandbox／connector／egress、Projection route、organization／quorum／release／modified／partial |
| `HumanDecisionRuntime` | trusted single-human authorityに対する`decision/*`のidentity／Policy／proposal／base／disposition／duplicate／atomic CAS整合 | credential本人確認、ACL、organization代理、quorum／MFA、modified／partial／release approval |
| `SqliteProjectionStore` | supplied Ref snapshotのcurrent closure、derived query row、Analysis lineage／prerequisite availability、missing診断とtombstoned availability／count、source fingerprint | authorization、ACL／tenant isolation、exact replay、最新Refとの自動同期、objectの正本性、archive／recovery completeness |
| detached Assurance | signer / service が何を検査・主張したか | Claim 本文の真実 |
| archive checksum / restore | package 内 byte と graph の整合性 | sender identity、機密性、外部 copy の回収 |

checksum と OID は attacker authentication ではない。攻撃者が object graph と manifest を丸ごと作り直せる場合、
内部的に整合した別 archive を作れる。署名済み配布 profile は未定義である。

## local trust boundary

```mermaid
flowchart LR
    U["Untrusted input<br/>Blob / JSON / archive"] --> L["Resource limits"]
    L --> P["Strict parser"]
    P --> S["Concrete schema + local semantics"]
    S --> O["Canonical bytes + OID"]
    O --> CAS[("Filesystem ObjectStore<br/>immutable source of truth")]
    CAS --> C["Typed closure verification"]
    C --> LOW["Repository::update_ref<br/>trusted operator primitive"]
    C --> AI["CreativeAiRuntime<br/>preflight + AI proposal admission"]
    C --> HUMAN["HumanDecisionRuntime<br/>narrow decision admission"]
    LOW --> R[("SQLite Ref + reflog<br/>CAS + preconditions")]
    AI --> R
    HUMAN --> R

    A["Analysis / AI adapter"] -. untrusted derived output .-> U
    CLI["local CLI"] --> LOW
    Request["AI / Human request<br/>credential + project + opaque handle/permit"] --> APP["synapse-application<br/>process-local AI + narrow Human routes"]
    Control["Trusted profiles / candidate<br/>Executor + Clock"] --> APP
    AUTH["Injected Authenticator<br/>point-in-time session decision"] --> APP
    APP --> AI
    APP --> HUMAN
    CAS --> Proj["SQLite ProjectionStore<br/>disposable derived index"]
    R -. caller supplies consistent RefSnapshot .-> Proj

    classDef boundary fill:#fff4cc,stroke:#9a6700,color:#321;
    class U,A,CLI,Request,Control,AUTH,Proj boundary;
```

現状は local OS user と repository directory permission を信頼する。
production ingest は `synapse-core` を通す。`synapse-cas` の `*_unchecked` API は、
既に検証済みの canonical bytes を保存する低水準境界であり、外部入力へ直接公開しない。
initial application routeはcredentialをinjected `Authenticator`へ渡してからproject／handle／Repositoryを
lookupし、exact server mapとprocess-lifetime ACLからrouteする。malformed／unknown／forbidden projectは
同じpublic code／messageにするが、これはsemantic anti-oracleであってconstant-time／traffic-analysis保証ではない。
request planeはcredential、project selector、opaque execution handle／permitだけで、AI自身がRepository path、
actor／principal、authority OID、ContextPack、capability、base／target Ref、Clock、Executorを選べない。
Human Decisionでは同じapplicationがdirect humanを認証し、成功したAI publicationのnon-Clone
`AdmittedProposalHandle`をcontrol plane registrationでborrowする。reusable Human profileとserver-fixed
candidateからhuman、Human Actor／ContextPack Policy、canonical decision Ref、proposal／base chainを
`HumanDecisionAuthority`へ固定する。untrusted requestはopaque registration／permitだけである。

## validation stage

```mermaid
flowchart LR
    I[Structured input] --> V1["Ingest<br/>parse / schema / local semantics / OID"]
    V1 --> Store[ObjectStore]
    Store --> V2["Ref update<br/>target resolution / typed closure"]
    V2 --> Tx["Ref + reflog transaction"]
    Store --> V3["fsck<br/>stored byte identity + selected closures"]

    V1 -. does not require all targets present .-> Later["Objects may arrive in any order"]
    Later --> V2
```

- ingest は object 単体で判定できる schema、NFC、set、time、fixed-point 等を検査する。
- graph 参照先は object upload 順序を許すため、Ref 更新時に resolve して検査する。
- `fsck` は全 stored object の pathname、OID、canonical bytes を検査し、current Ref head の closure を辿る。
- Ref が一つもない `fsck` は stored Commit 全件を root にする。
- `fsck` は全 structured object へ JSON Schema を再適用するものではない。Core ingest / restore を通した object は投入時に schema 検証済みである。

## 既定 resource limit

| resource | default |
|---|---:|
| structured input / canonical bytes | 16 MiB |
| JSON nesting depth | 128（hard ceiling 256） |
| JSON nodes | 100,000 |
| one container の members / items | 50,000 |
| Blob | 512 MiB |
| closure objects | 100,000 |
| closure edges | 1,000,000 |
| closure depth | 512 |
| restore manifest | 64 MiB |
| manifest checksum file | 256 bytes |

現在の CLI からこれらを変更できない。CLI が structured file を読む段階で 16 MiB を超えた場合は
`resource_limit` ではなく `usage_error` を返す。

## 実装済みの durability / concurrency property

### ObjectStore

- Blob は bounded streaming で SHA-256 を計算する。
- structured object は schema 検証済み canonical bytes と OID を再照合する。
- temporary file を flush / `sync_all` した後、hard link の create-if-absent で OID path を公開する。
- parent directory を sync する。
- 同一 object を並行投入しても一つだけが `Created` となり、他は byte 一致を確認して `AlreadyPresent` となる。

### RefStore

- candidate Commit closure を SQLite write transaction の前に検査する。
- AI routeは`BEGIN IMMEDIATE`直後、Ref preconditionやstate readより前にtrusted Clockを再読し、
  Grantの`recorded_at` not-beforeとexclusiveな`expires_at`を再検査する。writer lock待機中の失効と
  backward clockはfail-closedにする。
- `BEGIN IMMEDIATE` 内で current head と expected head を比較する。
- generic Ref preconditionも同じ`BEGIN IMMEDIATE`内で検査でき、AI routeはContextPackの
  base Refとproposal target Refを一つのserializable updateとして扱う。
- Ref update と reflog append は同じ transaction で commit / rollback する。
- 同じ expected head の並行更新は一つだけが成功する。

### Application publication/ACL fence

- reusable `AuthorityProfile`とone-time `ExecutionRegistration`を分離し、一registrationから一permitだけを発行する。
- Core preflight decisionはsealed／non-Cloneだが、credential／ACL／TTLを表すapplication permitとは別である。
- permitはstateful、opaque、process-localで、application TTLとGrant expiryの早い方をexclusive
  deadline (`now < not_after`) に使う。
- permitをExecutor起動前にburnする。Executor／Clock／Core failureを含む全失敗で再利用しない。
- Executor完了後のAuthenticator再実行はFIFO fence取得前に行う。fence内でlive ACLとprofile suspensionを
  再検査し、live profileからauthorityを再構築してCore transaction完了まで保持する。
- 同一projectのACL／profile mutationも同じFIFO fenceを使う。これはsingle-process orderingであり、
  restart recoveryや複数process間のlinearizabilityを保証しない。
- successful AI publicationだけがinstance／project／proposal Ref/head-bound
  `AdmittedProposalHandle`を返す。Human registrationはこのnon-Clone handleをborrowし、reusable Human profileと
  server-fixed `HumanDecisionCandidate`へ束縛するため、requestはlow-level proposal Refやdecision candidateを選べない。
  handle evidenceはdenial後の修正版registrationへ再利用できるが、registrationとpermitはone-shotである。
- Human preparationは追加Executor／Core preflightを行わずone-shot `HumanDecisionPermit`を発行する。
  TTLはapplication deadlineだけで`now < not_after`を要求する。publicationはauthentication後にpermitをburnし、
  same FIFO fence内でlive ACL／profile／TTLを再検査して
  `HumanDecisionRuntime`のfull immutable validation／CASまで保持する。invalid Human permitも
  `execution_permit_invalid`を使い、Core errorはburn後のfinal publicationだけから透過する。
- AI／HumanともAuthenticator callbackはFIFO fence、application state lock、Repository lockの外で実行し、
  resultはpoint-in-timeなsession decisionである。Human publishは外部Executorがないため認証は冒頭の一回だけで、
  reauthしない。同じfenceで線形化するのはprocess-local ACL／profile mutationであり、queued requestに対する
  外部credential storeの即時revocation fencingは主張しない。permit TTLがwindowをboundedにし、production
  auth adapter／credential lease semanticsはdeployment責任である。
- application public errorは`authentication_required: authentication required`、
  `project_access_denied: project access denied`、`execution_permit_invalid: execution permit invalid`、
  `execution_failed: execution failed`、`configuration_invalid: application configuration invalid`、
  `service_unavailable: application service unavailable`へdetailを閉じる。Core codeはmatching permitを
  authenticated attemptがburnした後のfinal publicationからだけ透過する。AI preflight Core rejectionは
  configuration error、storage／resource failureはservice unavailableへ正規化する。Humanには別Core preflightがない。

### SQLite ProjectionStore

- callerが取得した一つのconsistent `RefSnapshot`とverified `FileObjectStore`だけを入力にする。
- explicit rebuildはcurrent headsから到達するobjectだけをindexし、orphan CAS objectを除外する。
- derived rowの全置換は一つのSQLite `BEGIN IMMEDIATE` transactionでcommit / rollbackする。
- schema versionとsource fingerprintを保存し、対応しないschema versionをopen時に拒否する。
- missing targetはRef-scoped issueへ残し、tombstoned targetは別availability／summary countとして区別する。tombstonedだけならclosureはcompleteになり得る。
- corrupt byte、schema/type不整合、cycle、resource truncationはrebuildを拒否し、直前のprojectionをquery可能なまま保つ。
- archive exportと同様にcooperative append-only ObjectStoreを前提とし、rebuild中のconcurrent GC／removalを許さない。一度presentと観測したobjectが消えた場合はmissingへ格下げせずrebuildを失敗させる。
- validなunrelated orphanはprojected row／fingerprintから除外する。ただしCore v0.1のTombstone解決はstore-wide Record scanを行うため、unreadable／digest-corruptなorphan Recordもfail-closedでrebuildを拒否し得る。
- Analysis replay `Ready`はinput、adapter implementation／configuration、transformのavailabilityだけを集約する。derived output／maskはblockせず、adapter実行可能性、environment、byte-identical／semantic replayを保証しない。

ProjectionStoreはpublication transactionに自動追随せず、RefStoreより古いsnapshotを表し得る。
そのためauthorization、Ref CAS、archive export／restore、recovery判断はprojection resultを使わず、
ObjectStoreとRefStoreを直接検査する。query consumerがfreshnessを必要とする場合は、使用した
`RefSnapshot`とprojection metadataのsource fingerprintをoperation境界で管理し、rebuild failureと
fingerprint／freshnessの古さをmonitorする。ただしその古さをauthorization判断に使わない。

`RefScope`はACL／tenant boundaryではない。特に`analysis_lineage`はglobalにnot-indexedと、index済みだが
selected Refからnot-reachableを別errorにするため、未認可callerには存在oracleとなる。serviceは
projectionを呼ぶ前にauthoritative project／Ref accessを検査し、認可後だけquery result／errorを返す。

### directory archive

- export は Ref と reflog を同じ SQLite read transaction で先に snapshot する。
- append-only ObjectStore の inventory を OID 順に copy し、object raw checksum と manifest checksum を保存する。
- Blob は streaming、structured object は bounded memory で処理する。
- restore は pathname を信用せず、regular file、checksum、claimed OID、schema、closure を再検証する。
- object phase の途中失敗は archive OID 集合の subset を残し得るが、Ref はまだ公開しない。
- 同じ archive の完全な subset なら restore を再開できる。Refs / reflog は最後に一 transaction で復元する。

archive は単一 file や圧縮形式ではなく directory である。現在のlayoutとvalidation ruleは
[Local directory archive profile](../spec/core/v0.1/archive-profile.md)にnormative draftとして定義する。

## Creative AI boundary

```mermaid
flowchart LR
    CP[ContextPack] --> I["Effective capabilities<br/>Actor ∩ Grant ∩ Policy ∩ Runtime"]
    DG[DelegationGrant] --> I
    P[Policy] --> I
    A[AI Actor] --> I
    Q["Credential + project + opaque handle"] --> APP["Application preflight<br/>one-shot permit"]
    I --> APP
    APP --> EX["Trusted Executor"]
    EX --> REAUTH["Re-authenticate<br/>FIFO fence + live ACL/profile"]
    REAUTH --> PR["Core full revalidation<br/>proposal/{agent}/{run}"]
    PR --> AH["AdmittedProposalHandle<br/>instance/project/ref/head bound"]
    AH --> HC["Human profile + server candidate<br/>one-shot registration"]
    HQ["Human credential + exact project"] --> HP["Human prepare / publish<br/>one-shot permit / FIFO fence"]
    HC --> HP
    HP --> G["HumanDecisionRuntime<br/>trusted single-human gate"]
    G -->|adopt unchanged| D["decision/*"]
    G -->|reject / defer / experiment| F[DecisionFeedback]
    PR -. unimplemented release gate .-> R["release/*"]

    I -. denied .-> X["ACL / Policy change<br/>erasure / egress<br/>physical effect"]
```

上図のうち、application preflightからone-shot execution／reauthorization／publicationへ至るinitial AI route、
admitted proposal handleからone-shot Human publicationへ至るnarrow route、各Core full admissionは実装済みである。
applicationはProjection queryを公開しない。

- authenticated actor、project、principal、human-gated base Ref、authority snapshot、固定ContextPackを
  Actor、AI Activity、ContextPack、DelegationGrant、Policy、candidate Commit間でcross-checkし、
  Activityのrequested capabilityを`AiExecutionAuthority`のpre-authorized exact setと一致させた後、
  全memberをActor × Grant × Policy × runtime capabilityで再交差検証する。
- Grantの期限、data class、project resource、writable Ref prefix、output byte上限を検査する。
- base Commitのcurrent snapshot Tree traversalがagent／principal Actor、Grant、Policyのexact OIDを含むことを
  検査する。ancestorだけのpresenceは認めず、principalはself-assertedなhuman／organizationからagentへの
  direct Stage 0 Grantに限定する。
- candidateは`commit_kind=checkpoint`、parentsがexactly `[ContextPack.base_commit]`でなければならない。
  既存proposalを更新するときもcurrent proposalをparentにせず、merge／proposal chainはStage 0対象外である。
- candidate／base closureと両snapshotのdeltaを照合し、base snapshotの全non-Tree objectを
  candidateでも保持する。Tree OIDだけは置換／再配置できる。admission Activity／固定ContextPack以外の
  新しいnon-Tree objectをActivity output closureへ束縛する。
  generated output closureはexplicit output rootsのclosureからContextPack selected input closureを差し引き、
  explicit rootだけを再追加する。input-only dependencyのbytes／assertion／typeをoutputとして二重評価せず、
  explicit outputにしたRecordはagent assertedなAnalysisResult／Claimだけを許す。Tree-only residualを除き、
  base snapshot外のselected inputをcandidate snapshotへ配置する場合もActivity output宣言が必要である。
  Tombstone、authority/control Record、nested Commitを拒否し、output上限はgenerated output closureと
  新規Tree bytesをOID dedupeして数える。
- current Activityが生成するClaimは`payload.ai_run_ref`を省略し、Activity `output_refs`からClaimへのedgeを
  provenance正本とする。同じActivityへのback-referenceはcontent-addressed OID cycleになり、旧runへの参照も
  current production provenanceを誤表示するため拒否する。
- AI routeは`proposal/*`だけを受理する。`decision/*`／`release/*`は
  candidateを読まず`human_gate_required`、他namespaceは`authorization_denied`で拒否する。
- ContextPackのexpected baseはproposal Ref CAS／reflogと同じSQLite transactionで検査し、
  mismatchを`stale_base`としてatomicに拒否する。
- error precedenceはRef lexical validation、namespace gate／proposal-only、candidate closure、
  残りのauthorization／初回expiry、transaction Clock guard、`stale_base`、target `ref_conflict`の順である。
  authorization／expiryを通過したrequestだけがlive base preconditionへ進む。
  unauthorizedかつstaleなrequestは`authorization_denied`となり、base stateを漏らさない。
- Policy selectorはexactまたはterminal `/**`だけをsegment boundaryで評価する。当該actionの
  unsupported selectorと評価不能なmatching conditional allowはfail-closedにする。
  ruleが適用されない場合は明示された`default_effect`を尊重し、fixtureと運用推奨はdenyである。
- initial authorization後もSQLite `BEGIN IMMEDIATE`直後にtrusted Clockを再検査し、lock待機中の
  expiry crossingやbackward clockで権限が延命されないようfail-closedにする。

同じapplicationから呼ばれる`HumanDecisionRuntime`は、AI routeが満たせないHuman Gateのうちnarrow
`decision/*` profileだけを実装する。成功したAI routeのopaque handleをcontrol planeがborrowし、
server-fixed candidateを登録した場合だけrequest permitを発行できる。handleはdenial後の修正版へ
再利用できるが、registrationとpermitはone-shotである。

- trusted `HumanDecisionAuthority`がauthenticated single human、project、Human Actor／Policy OID、canonical decision Refとexact current head、exact proposal Ref/head、Clockを固定する。AI Activity／ContextPack／Grant／base chainはtrusted proposalから解決する。untrusted updateはnew Decision Commit、DecisionFeedback、messageだけで、authorityやexpectationを選べない。
- Human Actorをself-asserted `actor_kind=human`へ限定し、authenticated reviewerがAI responsible principal、ContextPack／Grant asserter、Grant direct principalと同一であることを要求する。Human Actor／Policy exact OIDのbase snapshot presence、reviewerによるPolicy assertion、Policy OIDとContextPack policy binding、Policy scope、proposal chainのproject一致も検査する。
- Policy `publish`をdecision Refへ評価し、denyをgate／allowより優先する。このrouteが満たすのは`before_decision_ref`だけで、別gateは`human_gate_required`となる。golden Policyはdecision ruleなしのdefault denyなのでHuman runtime conformance fixtureではない。
- proposal Commitのtransitionをexactly one AI Activity、Decision Commitのtransitionをexactly one self-declared DecisionFeedbackへ限定し、Decision Commitの`bound_declaration_refs`をemptyにする。Activity／Grantは`before_decision_ref`を要求しなければならない。`adopted_unchanged`はproposal snapshot、`rejected`／`deferred`／`experiment_only`はbase snapshotだけを許す。`adopted_modified`／`partially_adopted`はhuman modification provenance未定義として拒否する。
- DecisionFeedback `source_refs`は根拠Evidenceのcitationとして許可するが、authorityやdeclaration bindingではない。Policy／Actor／Grantをciteしてもeffective authorityはtrusted exact OIDとprotected base snapshotだけから決まり、empty必須のCommit `bound_declaration_refs`とは意味が異なる。
- proposalのadoption-criticalなAI Activity／Context／Grant／output bindingを再検証し、新しいauthority/control Record、Tombstone、nested Commitを拒否する。ただしoriginal runtime capability／sandbox／execution-time Grant state／AI Actor×Policy intersectionやCreativeAiRuntime経由を証明しない。embedding serviceはCreativeAiRuntime admission済みと把握するproposalだけをtrusted Human authorityへ選ぶ責任を持つ。canonical decision lineageに同じproposalのDecisionFeedbackがあれば再決定も拒否する。
- ContextPack baseをtrusted canonical decision Ref/headへ一致させ、immutable authorizationをlive Ref readより先に完了する。その後backward Clock guard、proposal Ref precondition、decision/base target CAS／reflogを同一transactionで処理する。Human routeのproposal／decision競合は`ref_conflict`であり`stale_base`ではない。unauthorized requestへlive stateを開示しない。

Core admissionはpublication-time validationである。その手前のapplication routeはAuthenticatorと
trusted Executorをinjected dependencyとして順序付けるが、concrete identity方式やOS sandboxではない。

- `--actor`は任意のreflog metadataで、本人性を検証しない。
- 現CLIは`CreativeAiRuntime`を公開せず、allowed syntaxの全Ref namespaceを低水準
  `Repository::update_ref`経由で更新できる。local trusted operatorだけに提供する。
- applicationのprocess-lifetime exact project ACLとAI／narrow Human routeは実装済みだが、HTTP／JWT、durable／distributed ACL・permit、
  multi-process fence、Projection application route、organization／quorum／MFA、release approval、
  modified／partial workflow、Grant revocation、OS sandbox／egress／physical-effect enforcementは未実装である。
- Coreはopaque Blobの意味や用途からrequired capabilityを推論できない。embedding serviceがmodel／tool
  実行前にworkloadを分類し、exact capability setを認可する責任を持つ。
- 新しいTreeだけでbaseの全non-Tree objectを保持しながら再配置するcandidateはActivity output宣言なしでも
  proposalとして許可される設計上のresidualである。narrow human adoptionは可能だがreleaseは別workflowである。

## Tombstone と erasure

Tombstone は「target payload が利用不能である」という履歴を残す Record である。

- object deletion、key destruction、derived purge を実行する CLI はまだない。
- Tombstone が存在しても、既に export または third party へ渡した copy を消せない。
- digest 自体が既知内容の照合に使われ得るため、公開可否を別途判断する。
- current closure validator は Tombstone で解決した node を complete と扱う。live payload が必要な操作は availability を別途確認する。
- ProjectionのStage 0実装は、non-empty rebuildごとにRecord familyを件数／累積bytes上限内で一度だけ走査し、同じTombstone resolver snapshotとduplicate-head reportを全Refで共有する。永続incremental indexは未実装である。

## 現在の既知制限

| 領域 | 制限 |
|---|---|
| Ref target availability | Tombstone が missing root Commit を解決すると closure が complete になり得る。present Commit byte が必要な caller は別途確認する |
| fsck coverage | Ref がある場合は current heads が closure root。全 reflog head の closure や全 object の schema を再検査しない |
| archive round trip | export は current heads を事前検査する一方、restore は reflog の全 new head を検査するため、到達不能な過去 head の欠落が restore 時に判明する場合がある |
| crash recovery | write boundary ごとの process-kill / fault-injection test と startup temp cleanup は未実装 |
| concurrent export | cooperative append-only ObjectStore が前提。export-vs-update stress test と hostile filesystem replacement の完全 hardening は未実装 |
| export publication | destination existence check と final directory rename の間に race がある。portable no-replace primitive / lock policy は未決定 |
| portability | non-Unix の directory sync は no-op。hard-link 対応 filesystem が前提 |
| very large repository | export manifest size と inventory memory を export 側で事前制限しておらず、restore 上限を超える archive を生成し得る |
| media security | image bomb、malicious SVG、EXIF / ICC parsing sandbox は未実装 |
| confidentiality | archive と local Blob は Core 自体では暗号化されない |
| AI application identity | injected `Authenticator`のresultを信頼する。concrete credential format、JWT validation、MFA、credential database／rotationは実装しない |
| application state durability | exact project map、ACL、profile、registration、permit、FIFO fenceは一process内だけ。restartでhandle／permitは失効し、multi-process linearizabilityは未実装 |
| application semantic anti-oracle | malformed／unknown／forbidden projectは同じcode／messageだが、timing、traffic、filesystem side channelの同一性までは保証しない |
| tenant / project isolation | exact project mapとprocess ACLはあるが、shared multi-project CASのobject membership／classification resolverは未実装 |
| AI execution enforcement | trusted Executorはpermit後にだけ起動されるが、model process isolation、connector access、egress、SSRF、物理作用をOS／networkで阻止しない |
| opaque Blob capability | CoreはBlob byteの意味や意図した利用を判定しない。embedding serviceが実行前にexact capability setを認可する |
| AI Tree-only restructure | baseの全non-Tree objectを保持する限り、新規Treeによる再配置をproposalとして許可する。path-level preservationを意味しない |
| AI Grant lifecycle | immutable Grantの期限はtrusted Clockで検査するが、live revocation list／key rotation／delegation chain enforcementは未実装 |
| Human Decision identity | applicationはinjected Authenticatorとprocess ACLを使い、same-instance admitted proposalを要求するが、credential format／MFA／persistent membership DBを実装しない |
| admitted proposal handle | successful AI publicationのsame-process evidenceだけ。portable receipt、signature、restart後のproofではなく、別application instance／projectへ移せない |
| Human Decision scope | `decision/*`と4 dispositionだけ。organization／quorum、release、`adopted_modified`／`partially_adopted`は未実装 |
| low-level Ref primitive | `Repository::update_ref`とCLIはauthorizationを迂回できるため、untrusted AI callerへ公開してはならない |
| query projection | explicit rebuildのみで自動refreshはない。caller-supplied Ref snapshotの一貫性、cooperative append-only/no-removal運用、failed rebuildとfingerprint／freshnessの監視は上位層の責務。projectionの古さを認可へ使わない。SurrealDB adapterと全8-query／benchmark比較は未実装 |
| projection orphan scan | validなunrelated orphanはrow／query／fingerprintへ入らないが、store-wide Tombstone探索中のunreadable／digest-corrupt Recordはreachableでなくてもrebuildをfail-closedにする。`ProjectionLimits`はRecord件数／累積stored bytesをboundし、一つのsnapshotを全Refで共有する。永続cacheはなく、大規模storeではscan I/O／benchmark監視が必要 |
| projection query isolation | `RefScope`はquery filterだけでACLではない。Analysisのnot-indexed／not-reachable error差は存在を漏らし得るため、service authorization後だけ公開する |
| Analysis replay readiness | prerequisite availabilityのsummaryだけ。output／maskのavailability、adapter runtime互換性、determinism、byte-identical／semantic replayを保証しない |

## archive を扱うとき

- restore 先には専用の空 repository、または同じ archive の失敗 restore だけが残した exact subset を使う。
- restore 実行 user に不要な filesystem 権限を与えない。
- restricted data は archive 配布前に別途暗号化し、access / retention を管理する。
- checksum を sender signature と表示しない。
- export 後の copy は Core から remote erase できないことを利用者へ明示する。
- `fsck` failure 時は元 directory を保全し、object file や SQLite を手編集しない。

## 未実装の security layer

- concrete HTTP/JWT／MFA identity、credential database／rotation
- durable／distributed project ACL、profile、registration、permit、multi-process publication fence
- multi-project CAS membership／classification resolver
- Projection authenticated application route、organization／quorum workflow
- release approvalとmodified／partial human adoption workflow
- pre-execution sandbox、connector、external egress、physical-effect enforcement
- Grant revocation、credential／key rotation、delegation chain enforcement
- HTTP transport、TLS、rate limiting
- encrypted-at-rest payload、KMS、key rotation
- signed release / signed archive distribution profile、trusted timestamp
- malicious media decode isolation
- security issue の非公開報告窓口と response policy

repository を公開運用する前に、連絡先、embargo、supported versions、response policy を
root `SECURITY.md` として追加する。
