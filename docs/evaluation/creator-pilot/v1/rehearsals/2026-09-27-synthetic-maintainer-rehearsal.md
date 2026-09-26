# 2026-09-27 メンテナrehearsal（synthetic素材）

**これは実利用者評価ではありません。参加者はいません。** メンテナに代わってAI coding agentが、
このキットの手順に沿ってv0.9.0 binary（`synapse` / `synapse-local` / `synapse-present`、
`synapse --version`出力: `synapse 0.9.0`）を使い、`docs/tutorial/assets/`のsynthetic
tutorial画像（3枚とも1448×1086 px）だけで一連の操作を実際に行い、手順の抜け・誤りを
確認した記録です。実行環境はrepositoryのmain checkoutとは分離した一時作業pathで、
実施後にtemp repositoryとprocessを削除・停止しています。

## 実施内容

### CLI経路（`participant-task-ja.md` 1-A/2/3/4 相当）

1. `synapse init`で一時repositoryを作成。
2. `synapse creator-run`で`mural-treatment-01`（`--decision adopt`）を作成。
   出力は`disposition=adopt` `selected=true` `byte_identity=different`
   `comparison_warning="Different Blob bytes do not establish visual or physical change."`
   `fsck=clean`で、`participant-task-ja.md`が案内する確認項目と一致した。
3. `synapse creator-report`で同じsessionを読み返し、`disposition` `selected` `byte_identity`
   `rationale`がCLI出力どおり再表示されることを確認した。
4. 課題4（別セッション）として`mural-treatment-02`（`--decision defer`）を作成。
   `disposition=defer` `selected=false`となり、`facilitator-guide.md`のQ3
   check-point（adoptは`selected=true`、deferは`selected=false`でbaseを維持）と一致した。
5. 同じsession名（`mural-treatment-01`）で`creator-run`を再実行し、
   `creator_session_exists`で拒否されることを確認した。参加者向け文書の
   「すでに使ったsession名は再利用できません」という記述と一致する。
6. `synapse fsck`で両repositoryとも`issues=0`であることを確認した。

### ブラウザUI経路（`participant-task-ja.md` 1-B/2/3 相当）

1. 別の一時repositoryで`synapse-local --project mural=<path> --port 0`を起動し、
   印字された`http://127.0.0.1:<port>`を使用した（reverse proxyなし、loopbackのみ）。
   使用ツール: Playwright（`scripts/browser/node_modules`のChromium）、参加者に見立てて
   実際のフォーム操作を自動化した。
2. プロジェクト画面の「Creator session を開始」フォームでsession名・表示名・Subjectを入力し、
   3画像（Original／Current／AI output）を選んでProposalを作成した。
3. セッション画面の「画像を拡大して比較」を開き、比較画像A/Bとして「Current」「AI output」を
   選択した。3画像とも同じdecode後dimension（1448×1086）のため「重ねて表示」が選択可能になり、
   不透明度スライダーを75%へ動かして反映されることを確認した。
4. Rationale欄に理由を入力し、「Adopt」ボタンを押した。
5. **発見した点**: ボタンを押すとブラウザ標準の`window.confirm()`ポップアップ
   （`Project "mural" / Creator session "mural-treatment-ui-01" に adopt decisionを公開します。`
   ...`この操作を続けますか？`）が表示され、これを承諾しないとdecisionは送信されない
   （キャンセル相当だと`Decisionは送信されませんでした。`と表示される）。
   参加者向け文書はこのポップアップの存在に触れていなかったため、
   **`participant-task-ja.md`／`participant-task-en.md`のTask 1-B手順へ、
   ブラウザ標準の確認ポップアップが表示される旨と、OK／キャンセルの挙動を追記した。**
   あわせて`facilitator-guide.md`の準備節へ、ポップアップがポップアップブロッカーで
   隠れないことを事前確認する項目を追加した。
6. ポップアップを承諾すると、リクエストが送信され（`Human decisionを検証して公開しています…`
   のbusy表示を経て）、ページ再読み込み後に「完了」状態、
   `Disposition: adopt` `AI output selected: はい`、
   「記録した判断: AI outputを変更せず採用しました。」、
   「記録された理由: (入力したrationaleの原文)」が表示されることを確認した。
   CLI経路のcreator-reportで確認した項目と、意味的に一致する内容がUIでも読み返せた。

### Task 3b（公開用文章の下見）— CLI経路

1. 別sessionとして`mural-treatment-03`をCLIで作成し、rationaleに
   `PRIVATE-RATIONALE-CANARY-should-not-appear-in-public-bundle`という、後で検索しやすい
   canary文字列を入れた。
2. `synapse-present export <repo> <out> --session mural-treatment-03 --public`を実行し
   （`--presentation`は省略。`presentation.toml`を作らない最小構成でも`--public`は指定できる
   ことを確認した）、`exported=... visibility=public sessions=1`を得た。
3. 生成された`<out>`配下の全file（`index.html` `story.md` `manifest.json` `checksums.json`
   `projection.json` `target/public-projection.json`）に対して`grep -ril`でcanary文字列を検索し、
   **一致0件**を確認した。
4. `synapse-present preview <out>`が検証成功することも確認した。

### Task 3b（公開用文章の下見）— ブラウザUI経路

1. 別の一時repositoryで新しいsession（`mural-treatment-ui-02`）を作成し、rationaleに
   `PRIVATE-RATIONALE-CANARY-should-not-appear-on-presentation-page`というcanaryを入れて
   Adoptで確定した。完了後のセッション画面には、このcanaryが期待どおり表示されることを
   まず確認した（read-back自体は正しく機能している）。
2. プロジェクト画面の「公開用の制作ノートを作る」（`href="/projects/{project_key}/presentation"`、
   `crates/synapse-local-http/templates/project.html`のlinkと`presentation.html`の実装どおり）を
   開いた。このsessionを選択できる状態で、他のすべての入力欄（作品タイトル、概要、
   Creator表示名、Proposal agent表示名、セッションのタイトル、3画像のcaption、公開用の判断メモ）
   が空欄から始まっており、ページ本文（innerText）にもHTML全体にも、canary文字列は
   **一切含まれていない**ことを確認した。
3. この画面は`docs/presentation_sidecar.md`が説明するとおり、author-suppliedな公開用文章を
   ゼロから入力するためのformであり、private rationaleを自動転記しないことを実装レベルで
   確認できた。

## 見つけた差分・修正

- **修正**: 参加者向け課題シート（JA/EN）のTask 1-Bで、画像比較が「画像を拡大して比較」
  ダイアログの中にあり、比較画像A/Bを選ぶ操作が必要である旨を明記した（元の文面は
  「重ねて表示」だけに言及し、ダイアログを開く操作を省略していた）。
- **修正**: Adopt／Reject／Defer確定時にブラウザ標準の確認ポップアップが出ることを、
  参加者向け課題シートとfacilitator-guideの両方に追記した（元の文面は「確認画面」と
  だけ書いており、native `confirm()`であることが分からなかった）。
- **修正（親レビュー指摘への対応）**: 理解度確認の質問1・2・4が、期待される回答をほのめかす
  yes/no形式の誘導質問になっていた（例:「SynapseGit自身がその画像を作ったと思いますか」）。
  JA/EN両方の該当質問を中立的なopen-ended表現へ書き換え、`facilitator-guide.md`の対応する
  check-pointの見出し・文言も揃えた。
- **追加（親レビュー指摘への対応）**: 質問4（private rationaleと公開用文章の違い）は、
  それを確認する操作課題がなく推測でしか答えられない状態だった。参加者向け課題シート
  （JA/EN）へ「Task 3b」を新設し、ブラウザ経路では「公開用の制作ノートを作る」画面を、
  CLI経路では任意で`synapse-present export --public`の生成物を、公開せずに下見できるように
  した。下記のとおりこのTask 3bを実際にrehearsalし、rationaleが自動転記されないことを
  実装で確認した。
- **確認のみ（修正不要）**: `--decision defer`後に同じsession名を再利用できないこと、
  `byte_identity`の意味、adopt/defer時の`selected`値は、既存のkit文面と実際の出力が
  一致していた。

## 範囲外・未実施

- 実利用者・実参加者によるセッションは実施していない。
- axe／keyboard／screen readerによるaccessibility評価は行っていない
  （`publication-comprehension/v1`のcorpusとは別scope）。
- `presentation.toml`を実際に作成・入力してからの`synapse-present export --presentation`
  （author-supplied公開用文章を伴う完全なexport）は、このrehearsalでは行っていない
  （Task 3bは「rationaleが自動転記されないこと」の確認が目的で、`--presentation`なしの
  最小exportと、presentation画面の空欄状態の確認にとどめた）。

## 後片付け

- rehearsalで作成した一時repository・export先bundle（CLI経路・UI経路・Task 3b追加分とも）は
  scratchpad配下から削除した。
- 起動した`synapse-local` process（初回・Task 3b追加分とも）は`kill`で停止済み。
- Playwrightスクリプトと実行logはscratchpad配下に残しており、このrepositoryにはcommitしない。
