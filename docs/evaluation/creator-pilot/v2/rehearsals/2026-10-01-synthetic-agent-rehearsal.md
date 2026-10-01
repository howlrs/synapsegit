# 2026-10-01 rehearsal（合成素材・評価キットv2）

**これは実利用者評価ではありません。参加者はいません。** メンテナに代わってAI coding agentが、
評価キットv2の手順を通しで実行し、手順の抜け、画面と課題シートの食い違い、重大な問題の有無を確認した記録です。
参加者の理解度や負担は測っていません。

## 実施条件

| 項目 | 内容 |
|---|---|
| 版 | v1.0.0-rcの代わりに、#149（[PR #185](https://github.com/howlrs/synapsegit/pull/185)）と#155（[PR #189](https://github.com/howlrs/synapsegit/pull/189)、rehearsalの指摘を反映する前の版）を含むsourceから`scripts/package_release.sh`で作ったLinux x86_64 archive。`synapse --version`は`synapse 0.13.1` |
| archiveの確認 | `sha256sum --check SHA256SUMS`がOK。展開したarchiveの3 binary、`AI_AGENT_GUIDE.ja.md`、`docs/tutorial/assets/`の練習用画像を使用 |
| platform | Linux x86_64（WSL2）。macOS arm64は、#158のCI（test、archive作成、展開後のsmoke）で確認しており、このrehearsalでは扱っていない |
| エージェントの役 | AI coding agentが、同梱の`AI_AGENT_GUIDE.ja.md`の手順どおりにコマンドを実行 |
| 参加者の役 | 同じagentが、Playwright（Chromium、`ja-JP`、幅1280px）で`participant-task-ja.md`に書いた画面の名前だけを使って操作 |
| 素材 | 練習用の合成画像3枚（`mural-original.png`、`mural-current.png`、`mural-ai-proposal.png`） |
| path | 一時directory配下の`SynapseGit-pilot/`（`work`、`inbox`、`backup-1`、`restored-1`）。実施後に`synapse-local`を停止 |

## 課題ごとの結果

| 課題 | 結果 |
|---|---|
| 1. 準備 | `synapse init`と`mkdir`で、repositoryとInboxを作成した |
| 2. Inboxへの書き出し | `synapse inbox put … --generation-note-file … --format json`が成功し、`"decision_recorded": false`を返した |
| 3. 画面の起動 | ガイドどおりの`synapse-local --project work=… --import-root work=…`で`http://127.0.0.1:8787`を表示した |
| 4. 確認と判断 | 「セッション」ページに取り込み待ちの通知が表示された。「取り込む」→「確認する」で、`inbox-mural-pilot-1`、作成者名、対象、生成メモが入力済みのformが開き、提案を作成した。比較のdialogを開き、「ファイル内容の一致確認」（結果は「異なる」）と限界の説明を読んだ。理由を入力して「採用」を押し、確認dialogを承認して記録した |
| 5. 読み返し | 画面に記録した判断、理由、対象、作成者、記録時刻が表示された。`creator-list`と`creator-report`が同じ判断（`adopt`）と理由を返した |
| 6. backup | `synapse-local`の停止後、`synapse export`が成功した。空のpathへの`synapse restore`も成功し、`fsck`は`issues=0`だった。元と復元後の`creator-report --format json`は完全に一致した |
| 7. 2つ目の候補（任意） | `mural-pilot-2`を取り込み、「保留」を記録した。「保留した提案を改めて判断する」のリンクが表示された |

## 重大な問題の確認

| ID | 結果 |
|---|---|
| S1 記録の損失・破損 | なし。restore後の記録は元と一致し、`fsck`は問題を報告しなかった |
| S2 誤った判断の記録 | なし。2件の判断はどちらも画面のボタン操作でだけ記録された。エージェント役は判断を記録するコマンド（`creator-run`など）を実行していない |
| S3 主要な操作の失敗 | なし。Inboxへの書き出し、取り込み、判断、読み返し、backupを、進行役の手助けなしに完了した |

## 見つかった問題と処理

| # | 問題 | 分類 | 扱い |
|---|---|---|---|
| 1 | レビュー画面で、AI outputの出どころが保存値の`caller_supplied`のまま表示され、「。」の前に空白があった | 通常 | #155で修正（「外部で用意したもの」と表示） |
| 2 | 判断の確認dialogが「adopt decisionを公開します」と表示した。保存値と、外部公開と誤読しやすい「公開」が残っていた | 通常 | #155で修正（「「採用」の判断を記録します」） |
| 3 | 日本語画面の一致確認に、保存された英語の注意文（`Different Blob bytes …`）が表示された | 軽微 | #155で「技術的な詳細」へ移動。同じ意味の限界の説明は折りたたまずに表示する |
| 4 | 日本語のボタンだけが「Proposalを作成」で、周りの説明は「提案」だった | 軽微 | #155で「提案を作成」にそろえた |
| 5 | セッション一覧で、大文字小文字を区別するセッション名が大文字で表示された（表の行見出しにも大文字変換のstyleが効いていた） | 通常 | #155で修正（列の見出しだけに限定） |
| 6 | 取り込んで判断した候補が「取り込む」ページに残り、「セッション」ページの通知と件数も取り込み済みの候補を数えた | 通常 | [#186](https://github.com/howlrs/synapsegit/issues/186)（milestone `v1.0`）。[PR #191](https://github.com/howlrs/synapsegit/pull/191)で修正 |
| 7 | `creator-report`のテキスト出力で、生成メモがRustの内部表記（`CreatorGenerationNote { … }`）のまま出た | 軽微 | [#187](https://github.com/howlrs/synapsegit/issues/187)（v1.xへ延期。JSON出力は構造化されており、AIエージェント向けガイドはJSONを案内しているため） |

## キットへの反映

- `participant-task-*.md`の課題5に、画面が`inbox-`を付けたセッション名を提案すること、名前を変えた場合は
  エージェントに伝えることを追記した。
- 課題シートの画面の名前（「取り込む」「確認する」「提案を作成」「画像を拡大して比較」「ファイル内容の一致確認」
  「理由（任意）」「採用」「保留」）が、#155の後の画面と一致することを確認した。

## このrehearsalで分からないこと

- 実際の参加者の理解度、負担、有用性。質問（`questions-*.md`）には参加者がいないため回答していない。
- 人がAIエージェントに頼むときの言い回しの違いと、エージェントの製品ごとの差。
- macOS arm64での操作。
- 所要時間。操作は自動化しており、人の時間の参考にならないため記録していない。
