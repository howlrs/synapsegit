# 実制作 Pilot 開始前チェックリスト

このテンプレートは、[15分 壁画チュートリアル](../tutorial/README.ja.md)を終えたあと、
一つの実制作を数週間試すための準備用です。コピーして `[ ]` と記入欄を埋め、Pilot ごとに
private な作業場所へ保管してください。これは v0.11.0 の Stage 0 local Pilot の利用計画であり、
実制作の効果や安全性を立証する評価結果ではありません。

v0.11.0 の `creator-run` は、利用者が用意した original、current、AI output の3 fileを
取り込み、Human Decision を記録します。画像を撮影せず、pixel差分、視覚的・物理的な変化、
model の実行、共同作業を判定しません。実装境界は[使用ガイド](../usage_guide.md#pilotでの基本的な使い方)と
[CLI reference](../cli_reference.md)で確認してください。

## 1. Pilot の範囲を決める

- [ ] **Subject:** `[作品・壁面・区画・デザイン案件]`
- [ ] **期間:** `[開始日]` から `[終了日]`。最初は `[例: 2〜4週間]` に限定する。
- [ ] **担当者:** `[制作・記録・振り返りを行う人]`
- [ ] **問い:** `[この制作で、後から何を説明または再利用できるようにしたいか]`
- [ ] **対象外:** `[常時記録、精密な画像比較、共同レビュー等]`。現在の実装で扱わないことを記す。

### 記録する節目

常時記録ではなく、作業の意味が変わる節目を選びます。予定外の作業や記録できなかった範囲は
無理に埋めず、後追いで記録できた範囲と、記録できなかった節目を分けて残します。

| 節目 | 予定日または条件 | original／current／AI output の3 fileを用意できるか | 記録したい判断・制約 |
|---|---|---|---|
| `[例: 制作開始前]` | `[日付・条件]` | `[はい／いいえ]` | `[ ]` |
| `[例: 案の採否]` | `[日付・条件]` | `[はい／いいえ]` | `[ ]` |
| `[例: 不可逆な処置の前後]` | `[日付・条件]` | `[はい／いいえ]` | `[ ]` |
| `[追加]` | `[ ]` | `[ ]` | `[ ]` |

## 2. 画像の権利・同意を確認する

- [ ] 各入力fileについて、保存、local archive、必要なら限定した共有を行う権利または許可を確認した。
- [ ] 人物、顧客の成果物、場所、機密情報が写る場合、必要な同意と利用範囲を記録した。
- [ ] 同意がない画像、利用範囲が不明な画像、公開できない画像を明記した。
- [ ] 提案画像の出所を `[caller-supplied / その他の説明]` と記録した。SynapseGit は出所、権利、
  または model 実行を検証しない。

| file または節目 | 権利者・同意の確認先 | local archive に保存するか | 外部共有を許可する範囲 |
|---|---|---|---|
| `[ ]` | `[ ]` | `[はい／いいえ]` | `[共有しない／範囲]` |
| `[ ]` | `[ ]` | `[はい／いいえ]` | `[共有しない／範囲]` |

## 3. repository、archive、復元確認を準備する

- [ ] **repository path:** `[ ]`。既存の制作履歴を上書きしない新しいpathにする。
- [ ] **archive の保存先:** `[ ]`。repositoryとは別の、アクセスを制限した保存先にする。
- [ ] **バックアップ担当と頻度:** `[担当者・節目または頻度]`。
- [ ] **復元テスト先:** `[空の一時path]`。restore先は空にし、既存repositoryへrestoreしない。
- [ ] 最初の完了session後と終了時に、`synapse export`、空のtest pathへの`restore`、
  `creator-report`での読み直しを行う予定を入れた。

最初の完了sessionを `pilot-01` とした場合の確認例です。最初の3行に自分のpathを設定し、
session名も自分の値に置き換えます。archiveと復元先には未使用のpathを指定します。

```bash
SYNAPSE_PILOT_REPO="/path/to/repository"
SYNAPSE_PILOT_ARCHIVE="/path/to/new-archive"
SYNAPSE_PILOT_RESTORED="/path/to/empty-restore-target"
synapse export "$SYNAPSE_PILOT_REPO" "$SYNAPSE_PILOT_ARCHIVE"
synapse restore "$SYNAPSE_PILOT_ARCHIVE" "$SYNAPSE_PILOT_RESTORED"
synapse creator-report "$SYNAPSE_PILOT_RESTORED" pilot-01
```

`export`と`restore`の制約、archiveが暗号化・署名されないこと、restore先の空要件は
[CLI reference](../cli_reference.md#export)で確認します。復元後のreportが読めない、
またはarchiveの保存先を保護できない場合は、下の中止条件に従います。

## 4. private な記録と公開範囲を決める

- [ ] rationale、生成メモ、decision pin、内部identifierを誰が読めるか: `[ ]`。
- [ ] 通常のlocal archiveを置く場所とアクセス権: `[ ]`。
- [ ] 公開資料へ実名、実作品の画像、private rationale、生成メモを転記しないことを確認した。
- [ ] public bundleを作る可能性がある場合、共有するSubject、画像、説明文、宛先を事前に記した: `[ ]`。
- [ ] 共有前に生成物を人が確認する担当者: `[ ]`。

`creator-report --format json` は `private_local` のreportで、rationale、生成メモ、pinなどを
含み得ます。public bundleがprivate情報を一律に安全化するものとは扱わず、共有するexactなbytesを
確認します。

## 5. 中止条件と次の行動を合意する

次の表に、Pilotを止める条件と、止めた時に記録・保全する最小限の作業を決めます。

| 中止する条件 | Pilotを止めた時の行動 | 担当者 |
|---|---|---|
| 権利・同意または共有範囲を確認できない | 新しい入力を取り込まず、既存archiveと共有物の扱いを確認する | `[ ]` |
| private な記録を保護できない、または復元確認に失敗する | 新しい記録を止め、archiveと復元先を確認してから再開可否を決める | `[ ]` |
| 記録が制作の安全・納期・品質を損なう | 次の節目を記録せず、負担の内容を振り返りに残す | `[ ]` |
| `[この制作固有の条件]` | `[ ]` | `[ ]` |

## 開始の記録

- [ ] 開始日: `[ ]`
- [ ] 最初のsession名: `[ ]`
- [ ] このチェックリストを保管したprivateな場所: `[ ]`
- [ ] 終了予定日と振り返り担当: `[ ]`

期間の終了時には、[振り返り・継続判断テンプレート](./retrospective-template.ja.md)へ、
観測値、本人の意見、判断を分けて記録します。
