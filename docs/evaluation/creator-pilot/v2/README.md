# Creator pilot 評価キット v2

対象はv1.0.0-rc（`vX.Y.Z-rc.N`のrelease candidate）です。このキットは、v1.0の主対象である
「生成AIをきっかけに技術を使い始め、CLIをAIエージェント経由で使うクリエイター」が、
SynapseGitを自分の制作で使えるかを確かめるための手順書とテンプレート一式です。

参加者向けの通常手順では、自分のAIエージェントに頼んで候補（Original、Current、AI output）をInboxへ置き、
`synapse-local`の画面で確認して、採用・不採用・保留を自分で記録します。その後、記録を読み返し、
backupを作ります。実利用者3〜5人の評価は未実施です。v1.0.0の公開条件では、代わりにCodex、Gemini、Claudeが
3〜5件の隔離したAIモック環境で同じ主要フローを試行し、画面を目視確認します。詳細は
[2026-10-02 multi-AI mock evaluation](./rehearsals/2026-10-02-multi-ai-mock-evaluation.md)と
[v1.0 release plan](../../../v1_release_plan.md#リリース条件)を参照してください。これは実利用者の同意、人数、UX、
Human Decisionの評価ではありません。

[評価キットv1](../v1/)はv0.9.0の操作（CLIの`creator-run`またはブラウザの3ファイル取り込み）に
固定したまま変更しません。v2は別の利用モデルを評価するキットです。

## このキットに含まれるもの

| ファイル | 役割 | 主な読み手 |
|---|---|---|
| [`consent-ja.md`](./consent-ja.md) / [`consent-en.md`](./consent-en.md) | 実施前に読み、同意の範囲を選んでもらう説明と同意書 | 参加者 |
| [`participant-task-ja.md`](./participant-task-ja.md) / [`participant-task-en.md`](./participant-task-en.md) | 事前準備と操作課題。AIエージェントへの頼み方の例を含む | 参加者 |
| [`questions-ja.md`](./questions-ja.md) / [`questions-en.md`](./questions-en.md) | 理解度確認と振り返りの質問。回答例や正解は含みません | 参加者 |
| [`facilitator-guide.md`](./facilitator-guide.md) | 準備、進め方、check-point、観察項目、重大な問題の基準、見つかった問題の処理 | 進行役 |
| [`result-template.md`](./result-template.md) | 1セッション分の結果を記録する空テンプレート。未実施と実施済みを区別する | 進行役 |
| [`rehearsals/`](./rehearsals/2026-10-01-synthetic-agent-rehearsal.md) | 合成素材でのリハーサル記録と、AIモック環境での実運用試験の記録。いずれも実利用者評価ではありません | メンテナ・開発者 |

## 使い方

1. 参加者に`consent-*.md`を渡し、同意の範囲を選んでもらいます。同意がなければ実施しません。
2. 参加者に`participant-task-*.md`と`questions-*.md`（該当言語）を渡します。
   `facilitator-guide.md`のcheck-pointは参加者へ渡しません。
3. 進行役は`facilitator-guide.md`に従って準備・観察します。
4. セッション後、`result-template.md`をコピーし、1セッション1ファイルとして記録します。
5. 見つかった問題は、`facilitator-guide.md`の「6. 見つかった問題の処理」に従って、milestone `v1.0`へ
   入れるか、v1.xへの延期を理由付きで記録します。

## v1.0.0 AIモック試験

実参加者を使わないv1.0.0の公開前試験では、Codex、Gemini、Claudeが隔離したfilesystem、locale、viewport、
restart条件で主要フローを実行します。各試行は実行ログ、作成物の検査、localhost UIスクリーンショットの目視確認を
記録します。画面で使うDecisionは明示的なモック固定値であり、人の判断として記録・報告しません。実行結果は
[`rehearsals/2026-10-02-multi-ai-mock-evaluation.md`](./rehearsals/2026-10-02-multi-ai-mock-evaluation.md)にのみ
集約します。実参加者用の同意書、課題、質問票、結果テンプレートは、この試験には使いません。

## v1との違い

| 観点 | v1（v0.9.0） | v2（v1.0.0-rc） |
|---|---|---|
| 候補の作り方 | 参加者がCLIまたはブラウザで3ファイルを直接取り込む | 参加者のAIエージェントが`synapse inbox put`でInboxへ置く |
| 判断 | ブラウザまたはCLI（`creator-run --decision`） | 人がブラウザの画面だけで記録する。エージェントに判断させない |
| 読み返し | `creator-report`、セッション画面 | セッション画面、エージェント経由の`creator-list`／`creator-report` |
| backup | なし | 「管理」ページまたはエージェント経由の`synapse export` |
| platform | Linux x86_64 | Linux x86_64、macOS arm64 |
| 記録 | 理解度と使いにくさ | 理解度、使いにくさ、重大な問題の有無、Creator benefit metricの一部 |

## 範囲外

- 参加者の募集と日程調整はメンテナが行います。このキットは募集を代行しません。
- このキット自体は、理解度や有用性を測定済みとは主張しません。結果は参加者ごとの観察です。
- 成功率、生産性の向上、所要時間の目標値は主張しません。AIの採用率も成功の指標にしません
  （不採用・保留にも価値があるためです）。

## Scope note (English)

This kit evaluates SynapseGit v1.0.0 release candidates with the v1.0 target
users: creators who started using technology through generative AI and run
the CLI through an AI agent. The participant asks their own agent to place a
candidate in the import inbox with `synapse inbox put`, reviews it in
`synapse-local`, records Adopt, Reject, or Defer themselves, reads the record
back, and makes a backup. The kit ships a consent sheet, participant tasks,
and questions in English and Japanese; a facilitator guide with check-points,
observation items, the severe-problem criteria from the v1.0 release plan,
and a triage procedure; and an empty per-session result template. Kit v1
stays fixed to v0.9.0. Recruiting participants is the maintainer's work and
is outside this kit. Rehearsal records under `rehearsals/` use synthetic
material only and are not real-user evaluations.
