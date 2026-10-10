# SynapseGit project status

Audience: users、contributors、maintainers
Status: public project snapshot
Applies to: v1.2.0 stable release
Last verified: 2026-10-10

v1.2.0は、AIエージェントがInboxへ候補を置き、人がlocalhost UIで画像を確認して判断する
local single-user向けのreleaseである。v1形式は[互換性方針](./compatibility.md)に従って凍結済みで、
formal Core Stage 1は別の研究として続ける。
実利用者3〜5人の評価は未実施であり、人間のUX評価・参加者数・Human Decisionの実績は主張しない。
[#192](https://github.com/howlrs/synapsegit/issues/192)では、v1.0.0公開条件としてCodex、Gemini、Claudeが
4件の隔離したAIモック環境で主要フローを実運用試験し、localhost UIを目視確認した。Gemini・Claudeの独立reviewを受け、公開条件2を満たすと判断し、PR #197のmergeでIssueを完了した。合成素材でのrehearsalは実利用者評価の結果ではない。
両platformの手順は[v1.2.0 release notes](./releases/v1.2.0.md)を使う。

tagged v1.1.0 sourceのworkspace librariesには、generic regular-file application向けのsource-level C1
boundaryが含まれる。これはdeterministic mapper／bounded checkout、固定v1 JSON contract、sequential
Proposal／Decision workflow、host-authenticated one-shot approval、Proposal／Decision CAS前後を別SQLite
journalへ結ぶ明示的なrestart recovery／reconciliation orchestration、local public projectionである。
v1.2.0 release archiveは一つの`synapse` executableと相対compatibility symlinkで、Core、loopback UI、presentation commandを提供する。このboundaryのHTTP／CLI／browser UI、新binary、model
invocation、remote publish、durable identity／ACL、multi-process linearizability、production serviceを提供しない。

## 現在の成果

- v1.2.0は既存Core commandを保ったまま、`synapse serve`と`synapse present export|preview`を一つの実行fileへ統合する。`synapse export`は引き続きCore archive exportでありpresentation exportではない。Core／OID／archive形式とHuman Decision境界は変更しない

- [Creator pilot評価キットv2](./evaluation/creator-pilot/v2/)と合成素材でのrehearsal、および[4件のAIモック環境の実運用試験](./evaluation/creator-pilot/v2/rehearsals/2026-10-02-multi-ai-mock-evaluation.md)。後者はGemini・Claudeの独立reviewを経て受入済みである。実利用者評価は未実施で、公開後のフォローアップ候補として残る
- sessionのSubject、Creator、判断日時、読みやすいTimelineとtechnical detailsの折りたたみ
- project画面のセッション／取り込む／保守／履歴の分離と、未取り込み候補だけを数えるInbox通知
- strict JSON、canonical bytes、domain-separated OID
- concrete Record schemaとlocal semantic validation
- filesystem content-addressed ObjectStore、typed closure、Tombstone、`fsck`
- SQLite Ref compare-and-swapとreflog
- checksum-bound directory exportとverified restore
- original／current／caller-supplied AI outputを取り込む3-file creator Pilot
- AI-attributed proposalと`adopt`／`reject`／`defer`のHuman Decision
- primary Blob OIDだけを比較する保守的なbyte-identity Analysis
- timeline、decision、evidence、replay prerequisiteを検査するcreator report
- `creator-report --format json`によるversion付きprivate-local JSON document。`--format text`と省略時は従来のtext reportを維持する
- `creator-list`による全sessionの未検証の概要（text／`synapsegit-cli-creator-list-v1` JSON）、commandごとの`--help`、主なerrorでの`hint:`行
- complete Creator sessionの**非公開の記録を保存（JSON）**。操作時に既存の認証付きsession detailを改めて検証して取得し、`{"state":"complete","report":{...}}`を非公開JSONとして保存する。pending／incompleteには表示せず、再取得した記録の検証に失敗した場合は保存しない。理由、`generation_note`、annotation／pin、internal ID、source lineageを含み得るため、public bundleやrepository backupとは別であり、CLI JSON documentと交換可能ではない
- `creator-run --generation-note-file`によるprivate・user-declared生成メモの記録。通常archive／local reportには残るが、public bundleへは出力しない
- project、session、evidence、画像を読むloopback-only localhost UI
- header選択式の日本語・英語localhost UI。選択はbrowser cookieに保持し、対応する`Accept-Language`、日本語の順で解決する。利用者入力・保存済みtext、API identifier、error codeは翻訳しない
- pending／complete sessionで、表示可能な2画像の選択、全体表示／100%／200%拡大、同寸法画像の重ね表示と0〜100%の不透明度、keyboard操作、狭い画面の縦配置を提供するread-only比較ビュー（位置合わせ・差分解析は行わない）
- プロジェクト画面では未検証のbounded（最大200件）セッション概要をレビュー待ち優先・最近のRef更新順で表示し、状態／判断で絞り込める。概要にはSubject、Creator、判断、ordering time／time basis、派生元を含み、理由本文は表示しない
- 一覧外のsessionは、JavaScriptを有効にした**名前でセッションを開く**で完全一致の名前から開ける。1〜64文字の小文字開始の英数字・ハイフンだけを受けるcase-sensitive strict slugであり、検索や一覧の上限拡大ではない。存在しない名前は既存のsession-not-found errorになる
- 完了セッションには派生先への逆リンクを表示し、取り込みとセッション一覧を保守操作より先に配置する。fsck／archive操作の確認強度は変更しない
- boundedな三file importとsame-process Human reviewを行うlocalhost creator UI
- 任意の`--import-root PROJECT=INBOX`で有効になる、script出力のboundedな確認・一時保持・明示的な新規Proposal取り込み。CLIの`synapse inbox put`は、repositoryを開かず判断も記録せずに、この形式の候補を書き出す（AIエージェント経由の既定の経路）
- 検証済みの中断Proposal／完了Deferから3画像を引き継ぎ、元の判断を変更せず別sessionで再レビューする操作
- CLIの`creator-run`は取り込みとHuman Decisionを別実行へ分けず、同一processのone-shot authorityで一回だけ判断する。候補の事前確認はInbox、再検討はlocalhostの新session再レビューを使う
- v0.8.0の取り込み前ローカル画像プレビュー、サイズ・選択数表示、ファイル解除、UTF-8バイト上限の即時feedback、送信中の入力固定
- v0.8.0のadopt／reject／deferの結果説明・確認、理由のUTF-8バイト数feedback、送信中の入力固定と、完了画面での記録された理由の表示（判断の変更・再開は不可）
- current creator Ref／headと推奨actionを表示するread-only incomplete-session diagnostics
- exact project確認、server-fixed limit、process-local job pollingを持つlocalhost `fsck` UI
- exact project確認と論理archive slugだけを受け、server-owned archive rootへCoreのbounded
  atomic no-replace publicationを行うlocalhost archive export API
- logical archive slug、exact project確認、開始前確認、job pollingを持つlocalhost archive export UI
- exact target project／empty-target確認と論理archive slugだけを受け、server-fixed limitで
  Coreのexact-subset archive restoreを実行するlocalhost archive restore API
- Refsとreflogが空の表示中targetに限定し、一覧slug、exact target key、empty-target checkbox、browser確認、
  queued/polled operation、report一致確認の案内を持つlocalhost archive restore UI
- process-local authenticated AI routeとnarrow Human Decision library boundary
- bounded regular-file manifestをRef更新なしでdeterministicなnested ManifestTreeへ変換する
  `synapse-artifact` mapperと固定`generic-artifact` v1 application contract
- profile-owned repositoryをtrusted bootstrapし、exact current Decision headごとに一つのactive Proposal、
  adopt／reject／defer Decision、そのDecisionをverified accepted baseにした次Proposalを記録する
  sequential `synapse-artifact` workflow
- authenticated host actor／session、project ACL epoch、exact Proposal／Decision binding、Decision intent、
  expiryへ束縛され、Decision object／Ref mutation前にburnされるopaque approval
- trusted `DurableProposalBinding`のlive Proposal／Decision Refをproject fence内で確認し、
  ordinary one-shot Human registrationを作り直すrecovery registration
- Proposal CAS前のprivate intent、verified publication後のopaque `ReviewId`、Decision CAS前のexact intent、
  bounded review state、checkout-verified terminal outcomeを別SQLiteへ保存する`synapse-artifact-journal`と、
  auth／ACL後にlive Ref／reflogを照合してcrash windowを収束させるexplicit durable orchestration
- 一つのRef snapshotとselected `site`だけを読み、path／tree／authority／byte上限とmanifest digestを
  fail-closedで検査してpartial resultを返さないgeneric artifact checkout
- verified Ref snapshotから再構築できるSQLite ProjectionStore
- existing CASをread-onlyで扱い、checkpoint済みRef SQLiteのdigest検証付きprivate stable copyから、
  人／AI向けのcanonical JSON、Markdown、JavaScriptなしHTML、manifest、checksum、Synapse／GitHub target
  layoutをlocal生成する`PublicProjection`／`synapse-present`
- complete generic Decisionをbounded checkoutからのみ投影し、pending／incompleteからauthority情報を
  除いたversioned generic-artifact canonical JSON／Markdown／script-free HTML／local target bundle
- complete adopt／reject／deferとincomplete-onlyを混ぜずに固定したpublication理解度評価コーパス、
  machine-readable質問／oracle、privacy canary、静的accessibility baseline
- 英語tutorialから続けるCreator workflow／public-text／privacy/trustのfocused documentation path、および実制作Pilot用の日本語開始前チェックリストと振り返りテンプレート
- GitHub projectionとGit identity/importは設計済みで、GitHubをobject／Ref／reflogのauthorityにしない。Git importer、GitHub App、remote publish、hosted serviceは未実装
- Linux x86_64 GNUとmacOS arm64向けv1.0.0 archive、checksum、build attestationを[GitHub Release](https://github.com/howlrs/synapsegit/releases/tag/v1.0.0)で公開済み。両platformの公開後download／checksum／attestation／archive smokeは[tag workflow](https://github.com/howlrs/synapsegit/actions/runs/37026671551)で確認済み
- v1.xでの読み取り互換とv1形式の凍結（`sg-oid-v1`、Core record schema、archive profile、Inbox manifest v1、CLI JSON）。公開済み全19版が書いたrepositoryとarchiveをfixtureとして固定し、CIで読み取り・restoreを検査する（[互換性方針](./compatibility.md)）
- 既存repository向け操作は未作成・不完全なrepositoryを拒否。新規作成は`synapse init`／`creator-run`、または空directoryを登録した`synapse-local`で可能
- tracked Bash fence、Cargo direct-dependency図、OpenAPI revision registry、archive／generation browser flow、publication HTMLのrelease gate

実装範囲の詳細と根拠は[documentation index](./README.md#現在地)を参照する。

## Creatorの記録と試作（v0.8.0）

Issue [#79](https://github.com/howlrs/synapsegit/issues/79)〜[#82](https://github.com/howlrs/synapsegit/issues/82)の
生成メモ、判断ピン、派生セッション、公開用文章フォームは[PR #83](https://github.com/howlrs/synapsegit/pull/83)で統合済み。
v0.8.0配布binaryに収録される。使い方は[Creator操作ガイド](creator_workflow.md)を参照する。

- 生成メモは利用者申告としてexact候補に束縛し、判断理由とは分けて表示する。
- 画像ピンは全体のHuman Decisionと同時にprivateで保存し、部分採用を表さない。
- 同じprojectのcompleteからOriginal／Currentを再利用して別sessionを作る。新しいidentityとreviewを持ち、元AI outputのCurrentへの昇格や新しい観測を意味しない。
- 通常取り込みのcompleteに限り、公開用文章を空欄から入力して`presentation.toml`を出力できる。

privateメモ・ピン・固定した派生元履歴は通常のarchive／restoreで保持される。
**派生sessionの公開v1出力は未対応**で、sidecarフォームとbundle exportは拒否する。
completeな派生を含む全件exportも拒否するが、通常sessionの明示選択と既存v1検証は維持する。
派生公開を実装するには、private lineageを漏らさず再利用の意味を保持する新しい公開profileが必要である。
[統合headのCI](https://github.com/howlrs/synapsegit/actions/runs/34325081274)ではRust・仕様・文書と
Chromium 34件が成功し、独立レビューで検出した公開v1の誤表示も修正済み。

## 現在の利用対象

RCの主な評価対象は、生成AIをきっかけに技術を使い始め、CLIをAIエージェント経由で使うクリエイターである。
人がlocalhost UIで判断する利用モデルを[AIエージェント向けガイド](./ai_agent_guide.ja.md)で案内する。
captureや継続session編集は未実装であり、multi-user serviceは提供しない。v0.3.0で導入され
v0.4.0にも収録されるlocalhost UIは三file importと単一proposalのreviewを行えるが、AI outputはcaller-suppliedで、
pending reviewはprocess restartを越えて復元できない。ただしProposal closureを検証できるrestart後の中断と、完了したDeferは、記録済みの3画像を新しいsessionへ引き継いで改めて判断できる。元の判断は復元・変更しない。restart後等のincomplete sessionを
read-onlyで診断し、明示確認したbounded `fsck`をbackground jobとしてpollできる。表示したRef／headから
authorityを再構築せず、自動resume／cleanupも行わない。job stateと`last_fsck`はprocess-localである。
`synapse-present`は作者外の評価者がOriginal／Current／AI-attributed proposal／Human Decisionと
byte-identity-onlyの限界を読めるderived bundleを生成する。source-private rationale、internal Actor ID、
repository path、raw assetを含めず、public noteは別途author-suppliedとして区別する。GitHub targetも
local generationだけで、online serviceやremote publicationではない。source SQLiteは直接openせず、
checkpoint済みで最大512 MiBのmain fileをprivate temporary copyへ二重digest検証で取り込む。sidecarまたは
copy中のsource変更は`read_only_source_busy`となり、exportが発見するcreator sessionは最大100件である。

generic artifact v1の`ReviewId`は認証済みlookup用locatorであり、authority、permit、Core receiptではない。
raw journal API自身はrepositoryを検査せずauthorityを再構築しない。上位のdurable artifact orchestrationは
trusted project configとjournalのserver-owned bindingを使い、lookup前にfresh authentication／ACLを確認し、
immutable Proposal、live Proposal／Decision Ref／reflog、manifest digestを照合して新しいApplication authorityを
組み立てる。old credential／admitted handle／approval／registration／permitを復元せず、Decisionはnormal
`HumanDecisionRuntime` full validation／CASを通り、terminal outcomeはbounded selected-site checkout後だけ確定する。
Coreとjournalは別transactionなので、crash windowはexact intentとexplicit reconciliationで収束させる。
exact project map、ACL、profile、permit、FairGateはprocess-localのままで、Creator Pilot／localhost UIのpending
reviewはrestart後にresumeできない。same-process pending authorityも引き続きnon-serializableである。
v1はcaller-supplied AI attribution／execution未検証だけを受け、verified execution modeとmodel invocationは提供しない。

## 未実装またはproduction blocker

- capture client、repeatable／calibrated capture workflow
- pixel registration、visual difference、physical change interpretation
- model／connector invocationとpre-execution OS sandbox／egress control
- archive restore失敗後のautomatic resume／cleanup／review recovery（list、両API、export UI、restore UIは実装済み）
- durable generic Rust boundaryのHTTP／Creator／localhost UI統合、automatic worker resume／cleanup、継続session編集
- HTTP/JWT／MFA、durable/distributed ACL・permit・publication fence
- organization／quorum／release approval、modified／partial adoption
- public multi-tenant cloud implementation、tenant isolation、operations
- GitHub／Synapseへのremote publish adapter、credential、destination diff、publication receipt
- raw asset／safe derived thumbnail publication
- 固定コーパスを使った実Human／zero-context AI理解評価と実accessibility評価
- SurrealDB adapterとbenchmark decision

## 配布上の現在地

| Item | Status |
|---|---|
| Localhost Inbox and fresh interrupted/Defer review | Included in v1.1.0; inspect retained Inbox bytes before a person supplies a decision; no source decision rewrite |
| Public repository | Available |
| v1.1.0 GitHub Release | [Release assets and notes](https://github.com/howlrs/synapsegit/releases/tag/v1.1.0) |
| Linux x86_64 GNU binary | Release archive for glibc 2.34+; verify against the v1.1.0 `SHA256SUMS` |
| macOS arm64 binary | Release archive built and smoke-tested on macOS 14; not signed or notarized |
| Source build from fixed tag | Available from `v1.1.0`; Rust 1.88+ |
| SHA-256 release checksum | The v1.1.0 `SHA256SUMS` lists both platform archives; verify before extraction |
| Build provenance attestation | Verify the v1.1.0 archives against `refs/tags/v1.1.0`; the tag workflow checks the digest, tagged commit, and GitHub-hosted runner |
| `synapse-present` binary | Included in v1.1.0; local generation only, with no remote publish |
| Generic artifact v1 Rust sequential/durable workflow and application contract | Included in tagged v1.1.0 source/workspace libraries; explicit local journal/recovery API, not exposed as HTTP/CLI/UI, a new binary, or remote publish |
| crates.io / GHCR / OS packages | Intentionally unavailable; GitHub Releases only |
| Source use, Fork, and redistribution terms | Custom source-available license available; not open source |

`v0.3.0`で導入され`v0.4.0`にも収録される`SynapseGit Local` binaryは、上記の三file import／review、dedicated read-only
incomplete-session diagnostics、bounded browser `fsck`を含む。review authorityとmaintenance
job stateはprocess-localであり、process restartを越えて再開できない。`synapse-present`も
v0.3.0で導入された三binary構成をv0.4.0 archiveで維持するが、生成物のremote upload／publishは行わない。
generic artifact C1 library／schema／local projectionはv1.1.0 tagged sourceのworkspace libraryであり、
archiveのbinary数や既存binaryのHTTP／CLI／UI capabilityを変更しない。source-available licenseの
production／distribution／brand制限も変更しない。

v1.0.0の公開日時、archive checksum、attestationの検証実績は、[公開時の固定snapshot](https://github.com/howlrs/synapsegit/blob/f2da8cf2c4ff78063899a471644bb9d3c3dc0dd9/docs/project_status.md#配布上の現在地)から確認できる。

## 次の優先順位

v1.0に向けた対象利用者、互換性の約束、範囲、リリース条件は[v1.0 release plan](./v1_release_plan.md)で決めた
（[#147](https://github.com/howlrs/synapsegit/issues/147)）。v1.0の必須作業はGitHub milestone `v1.0`で管理する。
次の研究・改善の優先順位のうち、2の実利用者評価はv1.0公開後に行い、3の派生セッション公開はv1.0の後に扱う。
6の追加platformは、v1.0ではmacOS arm64を対象にする。

1. 分離済みの[publication comprehension corpus](./evaluation/publication-comprehension/v1/)で、
   zero-context AI、実Human、axe／keyboard／screen reader理解・accessibility評価を実施する。
2. 実装済みlocalhost import／review／diagnostics／bounded `fsck`／archive browser controlsの
   実利用者による一連の操作の評価と、browser end-to-end回帰coverageを拡充する。
   v0.9.0向けの手順は[Creator pilot 評価キットv1](./evaluation/creator-pilot/v1/)、
   v1.0の利用モデル（AIエージェント経由のInbox、人による画面での判断、backup）向けの手順は
   [評価キットv2](./evaluation/creator-pilot/v2/)として整備済み（どちらも合成素材でのrehearsalは完了）。
   **実利用者によるcreator benefit評価は参加者の募集・実施を含めて未完了のまま**であり、キットの整備や
   AIモック試験だけでは完了しない。v1.0の公開条件は、[v1.0 release plan](./v1_release_plan.md#リリース条件)に
   記した3〜5件のAIモック環境での実運用試験と目視確認である。
3. 派生セッション公開の必要性を評価し、対応する場合は再利用意味を保持する新しい公開profileを設計する。
4. fixed-point Observation datasetとpixel-level adapterを別contractとして検証する。
5. durable admission transactionを含むproduction control planeを実装する。
6. 追加platformの再現可能なbuild／artifact smokeを整備する。

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
