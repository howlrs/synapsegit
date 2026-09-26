# Creator pilot 評価キット v1

対象はv0.9.0です。このキットは、SynapseGitが自分の制作に合うかを確かめたい利用者本人
（creator）が、実際にimport→比較→Adopt／Reject／Deferの判断→記録の読み返しまでを
一度たどり、理解度と使いにくさを確認するための手順書とテンプレート一式です。

[`publication-comprehension/v1`](../../publication-comprehension/v1/)が公開bundleを
作者外の第三者が読めるかを評価するのに対し、このキットは**creator本人**の一連の操作対象です。
自動採点器も新しいJSON schemaもありません。すべてMarkdownの手順とfill-inテンプレートです。

## このキットに含まれるもの

| ファイル | 役割 | 主な読み手 |
|---|---|---|
| [`participant-task-ja.md`](./participant-task-ja.md) / [`participant-task-en.md`](./participant-task-en.md) | 事前準備（対象version、3 binary、前提条件、3画像の役割、自分の画像を使う場合の準備）と操作課題 | 参加者（creator本人） |
| [`comprehension-questions-ja.md`](./comprehension-questions-ja.md) / [`comprehension-questions-en.md`](./comprehension-questions-en.md) | 自分の言葉で答える理解度確認の質問。回答例や正解は含みません | 参加者 |
| [`facilitator-guide.md`](./facilitator-guide.md) | 実施手順、各質問のcheck-point（期待される理解）、観察項目、記録・同意の扱い | 進行役（facilitator） |
| [`result-template.md`](./result-template.md) | 1セッション分の結果を記録する空テンプレート。未実施と実施済みを区別する | 進行役 |
| [`rehearsals/`](./rehearsals/2026-09-27-synthetic-maintainer-rehearsal.md) | メンテナによるsynthetic素材でのリハーサル記録。実利用者評価ではありません | メンテナ・開発者 |

## 使い方

1. 参加者に`participant-task-*.md`と`comprehension-questions-*.md`（該当言語）を渡します。
   `facilitator-guide.md`と各種check-pointは参加者へ渡しません。
2. 進行役は`facilitator-guide.md`に従ってセッションを準備・観察します。
3. セッション後、`result-template.md`をコピーして1セッション1ファイルとして記録します。

## 範囲外

参加者の募集、社外への送付、実際の実利用者評価の完了は、このキットの範囲外です。
このキット自体はcomprehensionを測定済みとは主張しません。[Project status](../../../project_status.md#次の優先順位)の
「実利用者によるcreator benefit評価」は、実施記録が積み上がるまで引き続き未完了として扱います。

## Scope note (English)

This kit is a documentation-only pilot procedure for the creator's own first
recorded decision: import the three tutorial images, compare them, record an
Adopt/Reject/Defer decision with a rationale, and read the saved decision
back. It ships participant task sheets and comprehension questions (English
and Japanese, answers withheld), a facilitator guide with separate
check-points and observation items, and an empty per-session result
template. There is no automatic scorer and no new JSON schema. Recruiting
real participants and completing a real-user evaluation are separate,
not-yet-done work; see the
[rehearsal record](./rehearsals/2026-09-27-synthetic-maintainer-rehearsal.md)
for the maintainer's synthetic-material rehearsal of this procedure only.
