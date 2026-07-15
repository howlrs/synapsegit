# SynapseGit 想定利用者別シナリオ資料

生成済み資料:

- [synapsegit_user_scenarios_ja.pptx](./synapsegit_user_scenarios_ja.pptx)
- [生成スクリプト](./generate_user_scenarios_pptx.py)
- [SynapseGit Core 使用ガイド](../usage_guide.md)
- [リポジトリREADME](../../README.md)

## 資料の用途

このPPTXは、導入候補者、現場責任者、制作リーダーへ、想定利用者別の課題とSynapseGit Coreの利用フローを説明するための資料である。

- 画家・壁画家
- 建築家
- 施工・修復担当
- デザイナーとCreative AIを含む制作チーム
- 後任、施主、所有者、コレクター、美術館等の二次利用者

画面モックではなく、利用構想、Pilot目標、人とAIの権限境界を示す概念図で構成している。最終slideはprocess-local authenticated one-shot AI route、same-instance admitted proposalに限定したnarrow Human application route、両Core admission、Analysis lineageを含むdisposable SQLite query projectionをlibrary実装済みとする。Human認証はpublish冒頭のpoint-in-time判断でpermit TTLが外部revocationとの差を限定する。一方、HTTP／JWT、durable／distributed ACL・permit、OS sandbox／egress、Projection application route、release／quorum、SurrealDB比較は未実装として区別する。PowerPoint、Keynote、LibreOffice Impress等で開けるが、環境によりフォントと改行を最終確認する。

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
- 10 slides
- shapeがslide境界外へ出ていないこと
- semantic title placeholderと極小text frameがないこと
- 日本語runに`ja-JP`と`Noto Sans JP`のEast Asian指定があること
- 図形にdecorative／alt metadataがあること
- `main`ブランチへのGitHub hyperlink
- PPTXを`python-pptx`で再読込できること

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
- 公開リンクは`branch: main`を明示する。

## 根拠資料

- [Core concept](../core_concept.md)
- [Stage 0 execution plan](../stage0_execution_plan.md)
- [Runtime architecture](../runtime_architecture.md)
- [Core Protocol v0.1](../../spec/core/v0.1/README.md)

この環境にはLibreOffice／sofficeがないため、配布前の最終レンダリング、PowerPoint Accessibility Checker、reading order、PDF変換後のlink確認はPowerPoint等で行う。

repositoryはpublicである。固定versionの説明資料として配布する場合は、PPTX内の`main` linkを
release tagまたはcommit permalinkへ更新し、link先の内容が後から変わらないようにする。
