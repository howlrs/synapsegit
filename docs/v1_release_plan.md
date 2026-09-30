# SynapseGit v1.0 release plan

Audience: maintainers、contributors、v1.0の評価者
Status: accepted plan（[#147](https://github.com/howlrs/synapsegit/issues/147)で決定）
Applies to: v1.0.0までの作業と、v1.x系列の互換性の約束
Last decided: 2026-10-01

v1.0は、Stage 0 previewを終え、生成AIを使って制作する個人が日常的に使える最初の安定版とする。
この文書は、v1.0の対象利用者、互換性の約束、対応platform、範囲、リリース条件を定める。
現在の実装状態は[Project status](./project_status.md)、配布手順は[Distribution guide](./distribution.md)を正本とする。

## 対象利用者と利用モデル

主対象は、**生成AIをきっかけに技術を使い始めたクリエイター**である。

- CLIは主にAIエージェント（CLIを実行できるAI assistant）経由で操作する。人がterminalで直接commandを打つことは前提にしない。
- 画像の確認とHuman Decisionは、人がlocalhost UIで行う。
- 一人の制作者が自分のcomputerで使う。multi-user、cloud、hosted serviceはv1.0の対象外である。

| 作業 | 主に行う主体 | 使う面 |
|---|---|---|
| 候補の生成と生成メモ（tool、model、prompt、intent） | AI（生成ツール、AIエージェント） | 生成ツール、Inbox manifest |
| 取り込みの準備 | AIエージェント | Inbox（manifest-last） |
| 候補の確認とAdopt／Reject／Defer | 人 | localhost UI |
| 記録の参照・集計 | AIエージェント、人 | CLIのJSON出力、localhost UI |
| backupと公開bundleの作成 | AIエージェント（人の確認の後） | CLI、localhost UI |

この利用モデルから、v1.0には次が必要になる。

1. **AIが誤りなく扱えるCLI**: 機械可読な出力、subcommandごとのhelp、安定したexit code／error code、errorの後の次の操作の案内。
2. **AI経由でもHuman Gateを保つこと**: 現在の`creator-run`は`--decision`が必須で、取り込みと判断を一度に行う。AIエージェントが実行すると、AIが選んだ判断が人のHuman Decisionとして記録される。v1.0でCLIの互換性を約束する前に、この扱いを決める。[#166](https://github.com/howlrs/synapsegit/issues/166)で、AIエージェントは`synapse inbox put`でInboxへ候補を置き、人がlocalhost UIで判断する経路を既定にすると決めた。
3. **人が読めるlocalhost UI**: 人が画像を見て判断する場として、protocolの用語を知らなくても読める画面にする。
4. **AIエージェント向けの操作ガイド**: 役割分担、手順、してはいけないことを1か所にまとめ、release archiveに同梱する。

## 互換性の約束

v1.xは次の二つを約束する。

- **読み取り互換**: v1.xのbinaryは、v0.1.0以降のすべての公開版とv1.xで作ったrepository、archive、creator sessionを読める。
- **v1形式の凍結**: 下表で「凍結」とした形式は、v1.xの間に意味と形式を変えない。変更が必要な場合は新しい識別子を追加し、旧形式の読み取りを維持する。

| 対象 | 識別子 | v1.0での扱い | 変更が必要なとき |
|---|---|---|---|
| Core OID profile | `sg-oid-v1` | 凍結 | 新しいprofile識別子を追加する。既存のOIDは再計算しない |
| Core record schema | [`spec/core/v0.1/schemas`](../spec/core/v0.1/schemas/) | 凍結（新しいrecord種別やextensionの追加は可） | 既存recordの意味を変えず、新しいrecord種別またはextensionで表す |
| Archive format | [local directory archive profile](../spec/core/v0.1/archive-profile.md) | 凍結 | 新しいarchive profileを追加し、旧archiveのrestoreを維持する |
| Inbox manifest | `synapsegit-import-inbox-v1` | 凍結 | 新しいversion識別子を追加し、v1の受け付けを続ける |
| Private report JSON | `synapsegit-cli-creator-report-v1` | 凍結（既存のadditive rule） | fieldの追加はv1のまま行い、削除・rename・意味の変更には`-v2`を使う |
| 公開bundle | publication profile v1 | 凍結（既存どおり） | 新しいprofileを追加し、v1 bundleの検証を維持する |
| Generic artifact contracts | `generic-artifact` v1、`generic-artifact-publication` v1 | 凍結（既存どおり） | 同上 |
| CLI | command名、引数、exit code、machine-readable error code | 安定 | 削除・変更の前に、少なくとも1 minor versionの非推奨期間を置く。text出力の文言と行は人向けで、変わり得る。機械処理にはJSON出力を使う |
| localhost HTTP API | `/api/v1`（`info.version`は`-draft`） | 約束しない | browser UIの内部契約とする |
| Rust crateのAPI | workspace crates | 約束しない | crates.ioへ公開しない |

OID凍結の根拠は、Rust実装と独立したJavaScript verifierがすべてのgolden fixtureでOID、canonical length、
canonical SHA-256について一致すること、および全公開版の読み取り互換testとする。
Core Protocol READMEが定める「第二の独立production実装」は、OID凍結の条件から外し、Stage 1の研究課題として残す。
仕様・文書への反映と、全公開版の読み取り互換testは[#165](https://github.com/howlrs/synapsegit/issues/165)で行う。

## 対応platform

| Platform | v1.0での扱い |
|---|---|
| Linux x86_64 GNU | prebuilt archive（現行どおり） |
| macOS arm64（Apple Silicon） | prebuilt archiveを追加する（[#158](https://github.com/howlrs/synapsegit/issues/158)） |
| Linux ARM64 | tagged source build。prebuiltはv1.x以降に検討する |
| Windows | 対象外 |

## v1.0の範囲

GitHub milestone [`v1.0`](https://github.com/howlrs/synapsegit/milestone/1)が、必須のIssueを示す。

### 必須

人が判断するlocalhost UI:

- [#149](https://github.com/howlrs/synapsegit/issues/149) project画面をsession一覧中心に再構成する
- [#150](https://github.com/howlrs/synapsegit/issues/150) session詳細とcreator-reportに、対象名・作成者名・判断の記録時刻を表示する
- [#151](https://github.com/howlrs/synapsegit/issues/151) 日時とTimelineを人が読める表示にする
- [#155](https://github.com/howlrs/synapsegit/issues/155) 画面の専門用語を平易にし、技術情報を折りたたむ
- [#159](https://github.com/howlrs/synapsegit/issues/159) 日本語表示で英語のまま残る欄名を直す

AI経由のCLI操作:

- [#157](https://github.com/howlrs/synapsegit/issues/157) CLIにsession一覧、subcommandごとのhelp、次の操作の案内を追加する
- [#166](https://github.com/howlrs/synapsegit/issues/166) AIエージェント経由の利用でHuman Decisionを人に残す方法を決める（設計。決定済み）
- [#171](https://github.com/howlrs/synapsegit/issues/171) `synapse inbox put`で、判断をせずにInboxへ候補を書き出す
- [#167](https://github.com/howlrs/synapsegit/issues/167) AIエージェント向けの操作ガイドを用意する

互換性・配布・リリース:

- [#165](https://github.com/howlrs/synapsegit/issues/165) v1形式の凍結を仕様に反映し、全公開版の読み取り互換を検査する
- [#158](https://github.com/howlrs/synapsegit/issues/158) macOS arm64のprebuilt archiveを配布する（Linux ARM64はv1.0の対象外）
- [#168](https://github.com/howlrs/synapsegit/issues/168) release手順でpre-release tag（v1.0.0-rc.N）を扱えるようにする
- [#169](https://github.com/howlrs/synapsegit/issues/169) v1.0の利用モデルに合う評価キットv2を作り、v1.0-rcで実利用者Pilotを行う
- [#164](https://github.com/howlrs/synapsegit/issues/164) READMEを「できること・始め方」中心に再構成する

v1形式の凍結前に方針を決める設計（実装はv1.x以降でもよい）:

- [#161](https://github.com/howlrs/synapsegit/issues/161) 採用した結果を次の制作段階の起点として記録するか
- [#162](https://github.com/howlrs/synapsegit/issues/162) 1回のreviewで複数のAI候補から1つを選べるようにするか

[#161](https://github.com/howlrs/synapsegit/issues/161)、[#162](https://github.com/howlrs/synapsegit/issues/162)、[#166](https://github.com/howlrs/synapsegit/issues/166)は、Core schemaやCLIの互換性に関わり得るため、いずれもv1形式の凍結前に方針を決める。
[#161](https://github.com/howlrs/synapsegit/issues/161)と[#162](https://github.com/howlrs/synapsegit/issues/162)をv1.0に含めない場合は、凍結したv1形式を壊さずに、新しいrecord種別やextensionとして後から追加できることを確認してから決める。

### 入れば良い（リリース条件にしない）

- [#148](https://github.com/howlrs/synapsegit/issues/148) project登録の保存とブラウザ起動
- [#152](https://github.com/howlrs/synapsegit/issues/152) session一覧のサムネイル
- [#153](https://github.com/howlrs/synapsegit/issues/153) 200件を超えるsessionの検索とページ送り
- [#154](https://github.com/howlrs/synapsegit/issues/154) 公開bundleの生成とpreviewをlocalhost UIで完結させる
- [#156](https://github.com/howlrs/synapsegit/issues/156) session名とarchive名の既定値の提案

AIエージェントはCLIでこれらの操作を代わりに行えるため、v1.0の利用モデルでは必須にしない。

### v1.0の後

- [#160](https://github.com/howlrs/synapsegit/issues/160) restart後も同じsessionのまま判断を続ける。AIエージェントがInboxへ候補を置く流れでは、人がlocalhost UIで確認するまでProposalを作らないため、v1.0では既存の再レビュー経路で足りる。
- [#163](https://github.com/howlrs/synapsegit/issues/163) 派生sessionの公開。新しい公開profileは、凍結した公開bundle v1と並べて後から追加できる。

## リリース条件

次をすべて満たした時にv1.0.0を公開する。

1. milestone `v1.0`のIssueがすべてcloseしている。
2. v1.0.0-rcを配布し、主対象に合う3〜5人の実利用者が評価キットで試用している。次の重大な問題がなく、見つかった問題はmilestone `v1.0`で解決済みか、v1.xへの延期を理由付きで記録している。
   - 記録の損失または破損
   - Human Decisionの誤った記録（AIの判断が人の判断として記録される等）
   - 主要な操作（Inboxへの候補の書き出し、取り込み、判断、振り返り、backup）を完了できないこと
3. 全公開版で作ったrepositoryとarchiveの読み取り互換testが、CIとrelease workflowで通る。対象とする版の範囲（pre-release tagを含むか）は[#165](https://github.com/howlrs/synapsegit/issues/165)と[#168](https://github.com/howlrs/synapsegit/issues/168)で決める。
4. Linux x86_64とmacOS arm64で、release gate（test、binary smoke、archive展開後のsmoke、checksum、build provenance attestation）が通る。
5. README、install guide、AIエージェント向けガイド、互換性方針、release notesがv1.0の内容に合っている。凍結した形式の文書から「draft」表示を外し、「Stage 0 preview」表示をやめる。
6. [SECURITY.md](../SECURITY.md)のsupported versionsがv1.xを示す。

## 変えないこと

- source-available license（[SynapseGit Source-Available License 1.0](../LICENSE)）と配布channel（GitHub Releaseのみ。crates.io、GHCR、container imageは提供しない）。変更する場合は別に判断する。
- loopbackだけで動くsingle-userのlocal application。SynapseGitはmodelを実行せず、AI出力はcaller-suppliedとして記録する。
- evidence、analysis、claim、Human Decisionを分ける原則。OIDは作者性、真実、権利、物理的な変化を証明しない。
- Human Decisionは人だけが記録する。

## formal Core Stage 1との関係

v1.0はproduct releaseの軸であり、[Stage 0 exit gate](./stage0_execution_plan.md#exit-gate)とは別に扱う。
exit gateの未完了項目（第二の独立production実装、Painting control dataset、Creator benefit metric、SurrealDBの判断）は、
v1.0の条件にしない。Creator benefit metricの一部は、v1.0-rcの実利用者Pilotで採取する。
Stage 1への移行判断は、v1.x以降の研究として続ける。

## 決定の記録

2026-10-01、メンテナが[#147](https://github.com/howlrs/synapsegit/issues/147)で次を選んだ。

| 論点 | 選んだ案 | 選ばなかった案 |
|---|---|---|
| 主対象 | 生成AIをきっかけに技術を使い始めたクリエイター（CLIはAI経由） | CLIを使わず画面だけで完結する制作者、CLIを直接扱うtechnical creator |
| 互換性 | 読み取り互換とv1形式の凍結 | 読み取り互換だけを約束する、第二の独立実装を待って凍結する |
| Platform | Linux x86_64とmacOS arm64 | Linux ARM64も配布する、Linux x86_64のみ |
| 実利用者の評価 | v1.0-rcで3〜5人の小規模なPilotを行い、条件にする | 条件にせず、v1.0の後に行う |
| CLIの変更手順 | 削除・変更の前に、少なくとも1 minor versionの非推奨期間を置く | 規則を置かない |

必須・入れば良い・v1.0の後の区分とCLIの変更手順は、上の決定から導いた案を、メンテナが同日に[PR #170](https://github.com/howlrs/synapsegit/pull/170)のreviewで承認した。区分を変える場合は、この文書とmilestoneを同じPRで更新する。
