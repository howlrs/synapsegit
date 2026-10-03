# SynapseGit 想定利用者別シナリオ資料

## AIを使うクリエイターへの説明画像

[![準備をAIに任せ、画像を見て自分で判断し、制作の経緯を後から見返す3ステップ](../assets/creator-journey.ja.png)](../assets/creator-journey.ja.png)

非プログラマ向けの説明には、操作名より先に「AIへの依頼」「自分の判断」「手元の記録」を示します。
上は2026-10-03にimagegenで作成した説明イラストです。実画面キャプチャではありません。
次の画像も、スライドや資料へ挿入できます。

- [元の画像・現在の画像・AIの候補から判断まで](../assets/creator-overview.ja.png)（[English](../assets/creator-overview.en.png)）
- [AIに準備を頼む制作の3ステップ](../assets/creator-journey.ja.png)（[English](../assets/creator-journey.en.png)）
- [制作メモと公開用文章を分ける](../assets/private-public-notes.ja.png)（[English](../assets/private-public-notes.en.png)）

## 既存のシナリオスライド

生成済み資料:

- [synapsegit_user_scenarios_ja.pptx](./synapsegit_user_scenarios_ja.pptx)
- [生成スクリプト](./generate_user_scenarios_pptx.py)
- [SynapseGit Core 使用ガイド](../usage_guide.md)
- [15分 壁画tutorial](../tutorial/README.ja.md)
- [リポジトリREADME](../../README.md)

対象version: **v0.9.0**（[project_status.md](../project_status.md)に対して確認日 **2026-09-27**）。
capabilityが変わる変更をmergeしたら、この資料と対象versionもあわせて更新する。

## 資料の用途

このPPTXは、導入候補者、現場責任者、制作リーダーへ、想定利用者別の課題とSynapseGit Coreの利用フローを説明するための資料である。

- 画家・壁画家
- 建築家
- 施工・修復担当
- デザイナーとCreative AIを含む制作チーム
- 後任、施主、所有者、コレクター、美術館等の二次利用者

画面モックではなく、利用構想、Pilot目標、人とAIの権限境界を示す概念図で構成している。「MECHANISM」slideは
壁画tutorialに基づく具体例で、Original reference／Current observation／外部で用意した候補（AI-attributed
proposal）の3画像を記録し、byte identityのみで比較し、人がAdopt／Reject／Deferを判断し、その結果を
`creator-report`・localhost UI・`synapse-present`のlocal publication viewで読み返せることを示す。この一連は
v0.9.0で今すぐ試せる。

シナリオslide（05〜10）はそれぞれ単独で読めるよう、下部に「今すぐ試せる（v0.9.0）」と「構想・未実装」を
並べたstatus stripを持つ。START SMALL（10）は6つの手順それぞれに同じ趣旨の小さなbadgeを付けている。

最終「CURRENT STATE」slideは、**今すぐ試せる（localhost限定）**ものと**未実装・構想のみ**のものを分けて示す。

- 今すぐ試せる: loopback限定のlocalhost creator UI（3画像import／review）、byte identityのみのAnalysis、
  `fsck`／archiveのブラウザ操作
- library実装済み（Rust API・利用者向けUIではない）: process-local authenticated one-shot AI／Human
  application route。CoreとApp routeのRust libraryとして実装済みだが、HTTP／CLI／browser UIには未公開
- 未実装・構想のみ: 汎用のcapture client、pixel registration・視覚差分、汎用（general-purpose）の
  creator application、本番運用向けHTTP／JWT・durable ACL・permit、release／quorum、SurrealDB比較

PowerPoint、Keynote、LibreOffice Impress等で開けるが、環境によりフォントと改行を最終確認する。

## 再生成

リポジトリrootで実行する。

```bash
python3 -m venv .venv
source .venv/bin/activate
python -m pip install -r docs/presentations/requirements.txt
python docs/presentations/generate_user_scenarios_pptx.py
```

出力先を変更する場合:

```bash
python docs/presentations/generate_user_scenarios_pptx.py \
  --output /tmp/synapsegit_user_scenarios_ja.pptx
```

## 検証

```bash
python docs/presentations/generate_user_scenarios_pptx.py --check
unzip -t docs/presentations/synapsegit_user_scenarios_ja.pptx
```

生成スクリプトは次を検証する。

- 16:9、13.333 × 7.5 inch
- 11 slides
- shapeがslide境界外へ出ていないこと
- semantic title placeholderと極小text frameがないこと
- 日本語runに`ja-JP`と`Noto Sans JP`のEast Asian指定があること
- 図形にdecorative／alt metadataがあること
- 対象versionのGitHub release tag（`v0.9.0`）へのhyperlink。`main`ブランチへは張らない
- PPTXを`python-pptx`で再読込できること
- 保守的なtext-overflow見積り（全角≈1em・半角≈0.55emでの行幅推定とword-wrap枠の高さ比較、
  slack 1.3倍）。実viewerでの描画測定ではなく、明らかな高さ不足だけを拾う粗いheuristic

`--check`で検証できないもの（人手確認が必要）:

- PowerPoint／Keynote／LibreOffice Impress等の実viewerでの`Noto Sans JP`表示、改行、reading order
- PowerPoint Accessibility Checker、PDF変換後のlink確認
- overflow見積りが捉えない微妙な折返し・行間の見た目

## ビジュアル規則

| 意味 | 色・形 |
|---|---|
| Plan | 紫青、角形 |
| Activity | 橙、実線 |
| Observation／Evidence | 青緑、角形・実線 |
| Analysis | 灰色、破線 |
| Claim／AI Proposal | 紫、角丸・枝 |
| Human Decision | 茶、太線・Human Gate |
| EvidenceGap／警告 | 赤、明示ラベル |

使用fontは`Noto Sans JP`である。PPTXにはfontを埋め込まないため、配布先に同fontがない場合はPowerPoint等でfont置換または埋め込みを行う。

## 内容を更新するとき

- 概念slideを実在する画面のようなmockへ置き換えない。実画面を追加する場合は、versionと
  read-only等の実装境界をcaptionへ明記する。
- `20秒`, `30秒`, `2分`, `100%`は実績値ではなく、必ずPilot UX／受入目標と表示する。
- 写真やAnalysisを物理的事実として表示しない。
- 作者性、現実、真正性、契約適合、永久保存、改ざん不能を保証しない。
- AI ProposalとHuman Decisionのレーンを統合しない。
- 「今すぐ試せる」と「構想／未実装」を混ぜない。localhost限定の機能を汎用製品機能のように書かない。
- 公開リンクは固定version（現在`v0.9.0`）のrelease tagを明示する。`main`ブランチへは張らない。

## 根拠資料

- [Project status](../project_status.md)
- [Core concept](../core_concept.md)
- [Core data model](../core_model.md)
- [Creator操作ガイド](../creator_workflow.md)
- [Stage 0 execution plan](../stage0_execution_plan.md)
- [Runtime architecture](../runtime_architecture.md)
- [Core Protocol v0.1](../../spec/core/v0.1/README.md)
- [15分 壁画tutorial](../tutorial/README.ja.md)

生成スクリプトは実viewerでの最終レンダリング、PowerPoint Accessibility Checker、reading order、PDF変換後のlink確認を行わない。配布前にPowerPoint、Keynote、LibreOffice Impress等で確認する。

repositoryはpublicである。PPTX内のGitHub linkは常に固定versionのrelease tag（現在`v0.9.0`）を指す。
対象versionを更新するときは、生成スクリプトの`TARGET_VERSION`と`CHECKED_ON`、この資料冒頭の対象version・確認日、
CHANGELOGのエントリを同じPRで更新する。
