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

## 見つけた差分・修正

- **修正**: 参加者向け課題シート（JA/EN）のTask 1-Bで、画像比較が「画像を拡大して比較」
  ダイアログの中にあり、比較画像A/Bを選ぶ操作が必要である旨を明記した（元の文面は
  「重ねて表示」だけに言及し、ダイアログを開く操作を省略していた）。
- **修正**: Adopt／Reject／Defer確定時にブラウザ標準の確認ポップアップが出ることを、
  参加者向け課題シートとfacilitator-guideの両方に追記した（元の文面は「確認画面」と
  だけ書いており、native `confirm()`であることが分からなかった）。
- **確認のみ（修正不要）**: `--decision defer`後に同じsession名を再利用できないこと、
  `byte_identity`の意味、adopt/defer時の`selected`値は、既存のkit文面と実際の出力が
  一致していた。

## 範囲外・未実施

- 実利用者・実参加者によるセッションは実施していない。
- `synapse-present export` / `preview`によるpresentation bundle生成・検証は、このrehearsalの
  対象操作（import→比較→判断→読み返し）には含めていない（`docs/presentation_sidecar.md`と
  `docs/creator_workflow.md`が別操作として案内している範囲であり、このキットの操作課題外）。
- axe／keyboard／screen readerによるaccessibility評価は行っていない
  （`publication-comprehension/v1`のcorpusとは別scope）。

## 後片付け

- rehearsalで作成した一時repository（CLI経路・UI経路とも）はscratchpad配下から削除した。
- 起動した`synapse-local` processは`kill`で停止済み。
- Playwrightスクリプトと実行logはscratchpad配下に残しており、このrepositoryにはcommitしない。
