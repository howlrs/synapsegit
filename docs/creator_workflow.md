# 制作メモを残し、次の案を試す

対象はv0.7.0タグ以降の現在のmainです。リリース済みv0.7.0には、このページの生成メモ・
判断ピン・派生セッション・公開用文章フォームは含まれません。
[source build](quickstart.md#1-build-する)で作成した`synapse-local`を
[localhost runbook](../deploy/local/README.md)に従って起動してください。

## 1. 画像と生成メモを取り込む

プロジェクト画面で、新しいセッション名、表示名、Original／Current／AI outputの3画像を指定します。
必要なら使用ツール、モデル、プロンプト、制作意図を入力し、プレビューを確認してProposalを作成します。
画像を選択しただけでは送信されません。AI outputは利用者が用意する画像で、SynapseGitはモデルを呼びません。

生成メモは任意の利用者申告です。保存後は変更できず、モデル実行や作者性の証明にはなりません。
文字数ではなくUTF-8 bytesで、ツール／モデルは各300、プロンプト8192、制作意図2048までです。
空欄のままでも従来どおり取り込めます。

## 2. 画像上のメモと判断を記録する

pending画面の「画像上の判断メモ」で表示可能な画像を選び、最大10個のピンを追加できます。
マウス／タッチ、キーボード、整数座標入力で位置を調整します。各メモは200 UTF-8 bytesまでです。
表示倍率を変えても画像全体に対する位置を保持します。表示・decodeできない画像にはピンを追加できません。

必要なら判断全体の理由も入力し、Adopt／Reject／Deferの説明を読んで確認します。
ピンと理由は一度のHuman Decisionで保存されます。ピン単位の採用や部分採用は行いません。
理由とピンには個別上限に加えてJSONリクエスト全体の8192-byte上限があり、画面で残量を確認できます。

Deferもそのセッションの判断を完了します。同じ判断の編集・再開はできません。
pending reviewはサーバープロセス内に限られ、再起動後の再開には対応していません。

## 3. 完了した記録を読み返す

完了画面では生成メモ、判断理由、ピンをそれぞれ確認できます。CLIでは次を実行します。

```sh
synapse creator-report /path/to/repo session-1
```

生成メモは`generation_note_user_declared`、有効なピンは`decision_pins_private`として表示されます。
未対応・不正なピン形式は`decision_pins=unavailable`と表示し、判断履歴の検証とは分けて扱います。
通常のCore archive／restoreは、これらのprivateメモと画像への対応を保持します。

## 4. 同じ参照画像で次の候補を試す

Adopt／Reject／Deferのいずれかで完了した画面から「この記録から次の案を試す」を開きます。
派生元とOriginal／Currentを確認し、新しいセッション名、表示名、新しい候補画像1点を指定して作成します。
元の生成メモや判断理由はコピーされません。新しいProposalについて通常のHuman reviewを行います。

再利用するのは同じproject内で検証したOriginal／Currentのexact bytesです。
Adopt済みでも元AI outputはCurrentになりません。再利用は新しい撮影・観測や同一人物／物理対象の証明ではありません。
新しいセッションは別のidentity・Refs・reviewを持ち、元の判断は変えません。
派生元の固定した履歴は新しい画面とCreator report APIで確認でき、archive／restore後も検証されます。
別projectからの再利用には対応していません。

## 5. 公開用文章を準備する

プロジェクト画面の「公開用の制作ノートを作る」から、**通常の3画像取り込みで作成した完了セッション**を選びます。
文章を空欄から入力・確認し、`presentation.toml`をダウンロードします。
privateメモは自動転記されません。文章の出力、bundle生成、外部共有は別の操作です。
詳しい手順は[公開用の制作ノート](presentation_sidecar.md)を参照してください。

| 操作 | 通常セッション | 参照画像を再利用した派生セッション |
|---|---|---|
| 生成メモ・ピン・Human Decision・完了画面 | 対応 | 対応 |
| 通常のCore archive／restore | 対応 | privateな派生元履歴を含めて対応 |
| 公開用`presentation.toml`のフォーム出力 | completeのみ対応 | 公開形式v1では拒否 |
| `synapse-present export`の公開bundle | completeを出力 | 公開形式v1では拒否 |

公開形式v1は画像の再利用を表せないため、派生Currentを新しい現況と誤表示しないよう拒否します。
completeな派生セッションを含む全件exportも失敗し、bundle出力先は作成されません。
同じprojectの通常セッションは`--session`で選択してexportできます。既存v1 bundleの検証は継続できます。
派生セッションの公開には、再利用の意味を保持する新しい公開形式が必要です。

## 保存契約と関連資料

- [生成メモの保存契約](../spec/application/creator-generation-note/v1/README.md)
- [判断ピンの保存契約](../spec/application/creator-decision-pins/v1/README.md)
- [派生元の保存契約](../spec/application/creator-source/v1/README.md)
- [CLIとerror code](cli_reference.md)
- [実装状況と未対応範囲](project_status.md)
