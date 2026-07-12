# SynapseGit glossary

Status label:

- **implemented**: current Rust workspace で動作する。
- **partial**: 明示したlibrary境界は動作するが、上位serviceや利用者flowが未完成である。
- **protocol**: Core v0.1 normative draft に定義される。runtime enforcement が未実装の場合を含む。
- **concept**: 設計・Pilot 上の概念で、v0.1 schema が直接表現しない場合がある。

## Storage and identity

| Term | Status | Definition |
|---|---|---|
| Core | implemented / protocol | creative lineage の immutable object、OID、graph、Ref、archive を扱う中核。UI、画像解析、network service そのものではない |
| Blob | implemented | original byte sequence。filename、media type、policy は digest に含めず、別 Record から参照する |
| Record | implemented | concrete schema と共通 RecordEnvelope を持つ immutable structured object |
| ManifestTree / Tree | implemented | single path segment から Blob / Record / child Tree への mapping。snapshot の root になる |
| Commit | implemented | parent sequence、snapshot Tree、transition / declaration refs を束ねる immutable object |
| OID | implemented / draft | `<family>:sg-oid-v1:sha256:<digest>`。object family と canonical content identity を示す |
| `sg-oid-v1` | protocol draft | Stage 0 の OID profile。第二の独立 production implementation gate 前で、まだ freeze 済みではない |
| Synapse Canonical JSON | implemented / protocol | strict input domain、UTF-16 key order、integer-only number、exact string preservation 等を定める structured identity encoding |
| content-addressed storage | implemented | OID を pathname key に immutable object を保存する ObjectStore。本書では「storage CAS」と呼ぶ |
| compare-and-swap | implemented | current Ref head が expected head と一致した場合だけupdateする操作。本書では「Ref CAS」と呼ぶ。SQLite実装は同じtransactionで追加Ref preconditionも検査できる |
| closure | implemented | Commit から parent、Tree、Record、Blob 等へ辿る required dependency graph |
| Ref | implemented | Commit head への mutable named pointer。object identity には含まれない |
| `expected_head` | implemented | Ref update caller が期待する current Commit OID。create 時は absent を指定する |
| reflog | implemented | Ref ごとの old/new head、actor metadata、message、時刻を残す append-only event history |
| source of truth | architecture | identity と archive 復元の基準。local Stage 0 では filesystem ObjectStore と Ref/reflog が基準で、ProjectionStore ではない |
| ProjectionStore | SQLite baseline implemented / comparison partial | verified ObjectStoreと一貫したRef snapshotから破棄・再構築する非authoritative query index。SurrealDB adapterと完全な8-query／benchmark比較は未実装 |
| `SqliteProjectionStore` | implemented library | current Ref closureだけを一transactionでderived SQLite rowへrebuildし、closure／timeline／Observation dependency／Analysis lineageをqueryするStage 0 baseline |
| `RefSnapshot` | implemented | projection rebuild等へcallerが渡す一時点のRef head集合。ProjectionStore自身はRefStoreを読まず、その一貫性とfreshnessはcallerが保証する |
| projection source fingerprint | implemented | Ref snapshot、reachable object availability／length、typed edgeを束縛する`projection-source-v1:sha256:...` metadata。authorization proofではない |
| `RefScope` | implemented | timeline／closure／Analysis queryを全Refまたは選択したRef名へ限定するprojection query filter。ACL／tenant boundaryではない |
| Analysis lineage | implemented projection query | AnalysisResultのadapter、ordered input、transform、derived Blob、typed mask、availability、selected Ref reachabilityを返す非authoritative view |
| `AnalysisReplayReadiness` | implemented projection query | input、adapter implementation／configuration、transformのavailability summary。`Ready`はexact replayを保証せず、derived output／maskはblock条件にしない |
| directory archive | implemented / draft | `manifest.json`、checksum、objects を持つ非圧縮 directory。単一 `.sg` file ではない |

## Creative lineage semantics

| Term | Status | Definition |
|---|---|---|
| RecordEnvelope | implemented / protocol | `record_type`、`entity_id`、`recorded_at`、`asserted_by`、`origin`、source refs、payload 等の共通 immutable wrapper |
| `entity_id` | implemented / protocol | 同じ概念対象の複数 immutable Record version をつなぐ安定 ID。Record OID とは別 |
| Actor | implemented | human、team、tool、AI と capability metadata を表す Record。本人認証そのものではない |
| Subject | implemented | 時間を通して追跡する物理・デジタル対象 |
| Activity | implemented | 実際に行った行為、session、AI run。Plan や Observation とは分離する |
| Observation | implemented schema | 条件付きで観測された Evidence。画像差分や物理的事実の確定ではない |
| Evidence | semantic role | Blob、Observation、資料等、Claim の根拠として参照されるもの |
| AnalysisResult | implemented schema | versioned adapter が生成する非権威的な派生結果。comparability、limit、mask 等を持つ |
| Claim | implemented | actor が Evidence / object へ与えた解釈・主張。存在しただけで真実にはならない |
| ClaimReaction | implemented | existing Claim への acknowledge、endorse、dispute、reject、withdraw、moderate 等の追加履歴 |
| EvidenceGap | implemented | 欠測、記録不能、不明を「変化なし」にせず明示する Record |
| Assurance | implemented schema | signature、receipt、external timestamp 等の detached 検査結果。target body を書き換えない |
| origin | implemented | self-declared、tool-recorded、inferred 等、Record がどのように生成されたかの分類 |
| As-recorded | concept | 観測・確認できた範囲を表す引き渡し表現。写真だけで無条件に As-built と呼ばない |

## Observation vocabulary

| Term | Status | Definition |
|---|---|---|
| CaptureProfile | implemented | `imported`、`repeatable`、`calibrated` と必要条件を定義し、可能な主張の強さを制約する |
| 定点 Observation / fixed-viewpoint Observation | concept / pilot | 近い station / viewpoint から繰り返す観測。「固定小数点」を意味しない |
| fixed-point / ScaledInteger | implemented / protocol | floating point を使わず `mantissa`、`scale`、`unit` で計測値を表す方式。「定点撮影」とは別 |
| comparable / partial / incomparable | implemented schema | Analysis が入力を比較できる範囲を表す状態 |
| changed / unchanged / ambiguous / unobservable | implemented schema | Analysis mask の四状態。欠測や遮蔽を unchanged へ潰さない |
| Plan–Previous–Current comparison | concept / pilot | 意図した状態、直前 Observation、現在 Observation を分けて比較する UX |

## Creative AI and decisions

| Term | Status | Definition |
|---|---|---|
| ContextPack | implemented schema / protocol / runtime input | base Commit / Ref、selected Evidence、Policy、Grant、制約をAI runへ渡すimmutable context。AI admissionはexpected baseとcurrent base snapshot bindingを検査する |
| Policy | implemented schema / protocol / proposal + decision admission | permission、prohibition、Human Gateのproject-local snapshot。AI admissionはcapabilityをaction/resourceへ、Human Decisionは`publish`をcanonical decision Refへ写像し、明示的`default_effect`を尊重する |
| DelegationGrant | implemented schema / protocol / proposal admission | principal、delegate、capability、project/resource、data class、expiry、output limit、writable Ref prefixの委任上限 |
| `Application<A,E,C>` | implemented process-local AI + narrow Human application boundary | AIではinjected `Authenticator`、single trusted `AiExecutor`、Clock、exact project ACL、Core preflight、one-shot execution、reauth／FIFO fenceを順序付ける。Humanではsame-instance admitted proposal、server-fixed candidate、one-shot registration／permitをfull `HumanDecisionRuntime` publicationへ接続する。Projection routeは持たない |
| `Authenticator` / `AuthenticatedSession` | implemented injected boundary | opaque credentialをserver-trusted actor sessionへ変換するtrait／result。project／handle／Repository lookupより先に呼ばれるが、JWT等のconcrete credential profile自体は実装しない |
| `ProjectSelector` / `RegisteredProject` | implemented process-local route | callerのproject selectorをserver-owned exact mapへ対応させる値／登録済みRepository route。caller文字列からfilesystem pathを組み立てない |
| `AiAuthorityProfileConfig` / `AuthorityProfileHandle` | implemented reusable trusted application state | actor、project、principal、base、authority OID、ContextPack、exact／runtime capability、target Ref名、side-effect classをserver側に保持するreusable profileとopaque handle。initial implementationではprocess restartを越えて永続化しない |
| `RegisteredExecutionHandle` | implemented one-time application state | trusted control planeがprofile generationとtarget Refのcurrent expected headを一実行へsealしたregistrationのopaque handle。一つのpermitだけを発行できる |
| `AiExecutionPermit` | implemented one-shot application state | Core preflight decision、authenticated actor、project、registration、exclusive TTLへ束縛されたopaque non-Clone permit。Executor起動前にburnされ、失敗後も再利用できない |
| `AiPublicationReceipt` | implemented AI application result | successful AI publicationのCore `AuthorizationDecision`と`AdmittedProposalHandle`を一緒に返すresult。`decision`／`admitted_proposal` accessorsと`into_parts`を持つ |
| `AdmittedProposalHandle` | implemented reusable process-local evidence | successful AI Core reflogから作られ、application instance／project／proposal Ref/headへ束縛されたopaque non-Clone handle。Human registrationはborrowするためdenial後の修正版へ再利用できるが、portable proof／signature／restart後の証拠ではない |
| `HumanAuthorityProfileConfig` / `HumanAuthorityProfileHandle` | implemented reusable trusted application state | exact project、direct human ID、canonical decision Ref、Human Actor OID、Policy OIDをcontrol planeが固定するprofileとopaque handle。replace／suspendはgenerationとproject security epochを進め、ready permitをfenceする |
| `HumanDecisionCandidate` | implemented trusted control-plane candidate | new Decision Commit、DecisionFeedback OID、bounded messageだけをserver側で固定する値。request payloadではなく、全OID／semanticsはpublication時にCoreが検証する |
| `RegisteredHumanDecisionHandle` | implemented one-time application state | admitted proposal evidence、Human profile generation、server-fixed candidate、canonical decision Refの登録時current headをFIFO fence内でsealするopaque non-Clone registration。一つのpermitだけを発行できる |
| `HumanDecisionPermit` | implemented one-shot application state | authenticated actor/session、exact project、ACL epoch、profile generation、registration、exclusive application TTLへ束縛されたopaque non-Clone permit。認証成功後のmatching claimでburnされ、invalid stateは`execution_permit_invalid`を使う |
| application semantic errors | implemented application boundary | `authentication_required`、`project_access_denied`、`execution_permit_invalid`、`execution_failed`、`configuration_invalid`、`service_unavailable`のdetail-free public errors。Core codeはauthorized AI／Human permit burn後のfinal publicationだけから透過する |
| `AiExecutor` / `AiExecutionContext` / `ExecutedAiProposal` | implemented trusted adapter boundary | applicationへ一つだけ注入されるexecutor、server-constructed execution context、generated proposal result。requestはexecutorやauthorityを選べず、OS sandbox／egress enforcementを意味しない |
| `AiExecutionAuthority` | implemented trusted library input | embedding serviceがproject route選択とauthentication後に、actor、project、principal、human-gated base Ref、authority snapshot、ContextPack、pre-authorized exact capability set、runtime capabilityを固定するbundle。Activity requestはexact setと一致する必要があり、untrusted publish payloadから分離する |
| `AiPublicationTarget` | implemented trusted Core input | preflight前にserver registrationが固定するproposal Ref、expected head、`none`／`project_internal` side-effect class。Executor resultからは変更できない |
| `AiPreflightDecision` | implemented sealed Core value | candidate-independent authorityとlive base／target expectationのread-only check結果。non-Cloneでpublication時にconsumeされるが、credential／ACL／TTLを含むapplication permitではなくRefも予約しない |
| `AiGeneratedProposal` | implemented narrow Core input | trusted Executor後にCore publicationへ渡す`new_head`、`activity_oid`、messageだけの値。authority／target／capabilityを含まない |
| `CreativeAiRuntime` | implemented library boundary / partial product integration | `preflight_proposal`と`publish_preflighted`でauthorityを実行前／publication時に分けて検査し、後者でcheckpoint／single-base-parent、base non-Tree preservation、current snapshot authority binding、restricted output delta、transaction-time expiryとCASをfull revalidationするRust API |
| `HumanDecisionAuthority` | implemented trusted library input | application control planeが認証・project access確認後にsingle human、Human Actor／ContextPack Policy、canonical decision Refとexact current head、exact proposal Ref/head、Clockを固定するbundle。humanはAI responsible principal／Context・Grant asserter／Grant principalと同一で、server-fixed new Commit／Feedback／messageから分離する |
| `HumanDecisionRuntime` | implemented library boundary / partial product integration | direct human／Policy／proposal chainとsupported dispositionを検証し、proposal preconditionとtrusted canonical decision/base headのtarget CASをatomicに処理するnarrow Rust API。methodは`publish_decision` |
| `DecisionDisposition` | implemented Human Decision value | runtimeが受理したDecisionFeedback disposition。Stage 0 admissionは`adopted_unchanged`、`rejected`、`deferred`、`experiment_only`だけを受理する |
| `HumanDecisionReceipt` | implemented audit result | successful `publish_decision`が返すdisposition、Actor、Policy、proposal、feedback、reflogのbinding |
| proposal Ref | implemented syntax / authorized application + Core route | `proposal/*`の未採用branch。initial application routeから`CreativeAiRuntime`へ到達するが、CLI `update-ref`は認証を迂回するtrusted operator primitiveである |
| decision Ref | implemented syntax / narrow application + Core route | `decision/*`の採用判断系列。AI routeは`human_gate_required`で拒否し、same-instance admitted proposalに限定したHuman application routeから`HumanDecisionRuntime`へ接続してsupported dispositionを記録する。CLIは低水準primitive |
| release Ref | implemented syntax / partial Human Gate | `release/*`の公開・引き渡し版。AI library routeは`human_gate_required`で拒否する。人の承認routeは未実装 |
| Human Gate | protocol / narrow decision implementation | AIによるdecision/release直接更新の拒否と、trusted single humanが満たす`before_decision_ref` admissionは実装済み。release、modified／partial、quorum、policy change、egress、erasure、physical effectは未実装 |
| DecisionFeedback | implemented schema / protocol / decision admission | proposal のadopt／modify／reject／defer等とhuman rationaleを残すRecord。narrow runtimeは同proposalのcanonical再決定を拒否する |
| `ref_conflict` | implemented | mutable Ref の current head が update request の expected head と異なる concurrency error |
| `stale_base` | implemented AI library boundary / protocol | ContextPackのexpected baseとlive base RefがずれたAI output。authorization後にbase preconditionとproposal updateを同じSQLite transactionでatomicに拒否する。unauthorized＋staleではbase stateを開示しない |
| `authorization_denied` | implemented admission boundary / protocol | identity、project、object binding、capability、Policy、snapshot/disposition、control delta、duplicate等がAI proposalまたはHuman Decision publicationを許可しないerror |
| `human_gate_required` | implemented admission boundary / protocol | AIのdecision/release直接更新、または現在のtrusted routeが満たさないPolicy gateを示すerror。Human Decision routeが満たすのは`before_decision_ref`だけ |
| proposal-only | implemented AI library boundary / protocol | `CreativeAiRuntime`がAI publicationを指定`proposal/*`へ限定するrule。baseの全non-Treeを保持するTree-only restructureもproposalに留まり、人の採用が必要である。現CLIの低水準`update-ref`が強制するという意味ではない |

## Availability and deletion

| Term | Status | Definition |
|---|---|---|
| present | implemented | object payload を読み検証できる |
| tombstoned | implemented | target byte は利用不能だが、Tombstone Record で履歴を解決できる |
| missing | implemented | object も Tombstone も解決できない |
| Tombstone | implemented schema / closure | target、理由、時刻、replacement / derivative refs を持つ deletion history。delete command 自体ではない |
| erasure / purge | planned | payload、key、derived copy を実際に利用不能化する operation。current CLI にはない |

関連資料:

- [Core データモデル](./core_model.md)
- [CLI reference](./cli_reference.md)
- [Security model](./security_model.md)
- [Core Protocol](../spec/core/v0.1/README.md)
