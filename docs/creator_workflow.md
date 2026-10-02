# 制作メモを残し、次の案を試す

English: [Creator workflow](./creator_workflow.en.md).

対象はv1.0.0です。生成メモはv0.11.0、判断ピン・通常の派生セッション・公開用文章フォームはv0.8.0で導入されました。中断／Deferからの新しい再レビューと任意のInbox取り込みも利用できます。
v1.0.0 release binaryを[install guide](install.md)から導入するか、
[source build](quickstart.md#1-build-する)で作成した`synapse-local`を
[localhost runbook](../deploy/local/README.md)に従って起動してください。

`synapse-local --import-root KEY=PATH` を使う場合、producerは`PATH/<slug>`へ3つの画像を書き、strict manifestを最後に書く。ブラウザはpathではなくslugだけを送る。確認を閉じるとprocess-private stagingは直ちに破棄され、Proposal作成は確認した同じstaged bytesだけを使う。
CLIやAIエージェントからは`synapse inbox put`でこの形式の候補を書き出せる。repositoryを開かず、判断も記録しない。判断は人がブラウザで行う。

## 1. 画像と生成メモを取り込む

プロジェクトの「取り込む」ページで、新しいセッション名、表示名、Original／Current／AI outputの3画像を指定します。

プロジェクト画面のセッション一覧は、レビュー待ちを優先し、最近のRef更新順で最大200件を表示します。Subject、Creator、状態／判断、記録されたordering timeとtime basis、派生元で内容を見分けられ、状態または判断で絞り込めます。これは同一Ref snapshotから読み取った未検証の概要です。詳細はセッション画面で通常の検証を行います。理由や生成メモは一覧に出さず、各セッション画面で確認します。派生元の画面には派生先へのリンクも表示します。

## 古いセッションを名前で開く

一覧は最大200件のままです。そこに表示されないセッションは、**名前でセッションを開く**に完全に一致するセッション名を入力して開けます。名前は大文字小文字を区別し、小文字で始まる1〜64文字の小文字英数字とハイフンだけを使います。これは一つの名前を直接開く操作で、検索や追加の一覧表示ではありません。存在しない名前は従来のsession-not-found errorになります。この操作にはJavaScriptが必要です。無効でも一覧は読めますが、名前で古いセッションを開くことはできません。

画像を比較するときは「重ねて表示」を選べます。Aを下、Bを上に置き、同じデコード後寸法の画像だけを共通の左上原点で0〜100%の不透明度として見ます。B自身の透明部分は保たれます。この表示は位置合わせや差分解析を行いません。
必要なら使用ツール、モデル、プロンプト、制作意図を入力し、プレビューを確認して提案を作成します。
画像を選択しただけでは送信されません。AI outputは利用者が用意する画像で、SynapseGitはモデルを呼びません。

生成メモは任意の利用者申告です。保存後は変更できず、モデル実行や作者性の証明にはなりません。
文字数ではなくUTF-8 bytesで、ツール／モデルは各300、プロンプト8192、制作意図2048までです。
空欄のままでも従来どおり取り込めます。

CLIでも`creator-run --generation-note-file <path>`で、同じprivateな生成メモをUTF-8 JSON fileから記録できる。
JSONには任意の`tool`、`model`、`prompt`、`intent` stringだけを指定し、通常archiveとlocal reportには残るがpublic bundleには出力しない。詳しい形式、上限、errorは[CLI reference](./cli_reference.md)を参照する。

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

完了画面では生成メモ、判断理由、ピンをそれぞれ確認できます。**非公開の記録を保存（JSON）**は、操作時に既存の認証付きsession detailを改めて読み、検証済みのcomplete response
`{"state":"complete","report":{...}}`をそのまま保存します。pending／incompleteではこの操作を表示せず、再取得した記録の検証に失敗した場合はfileを出力しません。存在する場合、Human Decisionの理由、`generation_note`、annotation／pin、internal ID、source lineageを含む非公開の記録になります。public bundleやrepository backupではなく、CLIのJSON documentと互換・交換可能な形式でもありません。共有前に内容と共有先を確認してください。CLIでは次を実行します。

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

### 次の制作段階・複数の候補（v1.0の範囲）

採用した結果に加筆して次の段階へ進む場合は、新しく撮影・書き出した画像をCurrentとして、通常の3画像取り込み
または`synapse inbox put`で新しいセッションを作ります。v1.0では、前のセッションとのつながりは記録されません
（v1.x以降に対応予定。[#178](https://github.com/howlrs/synapsegit/issues/178)）。

一度に複数の候補を作った場合は、候補ごとにセッションを作り（同じOriginal／Currentなら「この記録から次の案を試す」、
AIエージェントなら候補ごとの`synapse inbox put`）、1件ずつ判断します。1つのreviewで複数の候補から選ぶ機能は
v1.0にはありません（並べて比較する画面はv1.x以降に予定。[#179](https://github.com/howlrs/synapsegit/issues/179)）。

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
- [3画像を引き継ぐ再レビューの保存契約](../spec/application/creator-reuse-source/v1/README.md)
- [CLIとerror code](cli_reference.md)
- [実装状況と未対応範囲](project_status.md)

## 中断した提案・保留した提案を改めて判断する

再起動後の未完了セッションは元の判断を再開できません。ただしProposalと3画像を検証できる場合は、画面で画像を確認・ダウンロードし、「この提案を新しいセッションでレビューする」を選べます。確認画面で新しいセッション名を入力すると、Original／Current／AI outputを引き継いだ新しいレビューが始まります。

`Defer`で完了したセッションでは「保留した提案を改めて判断する」を選べます。確認画面と新しいレビュー画面では、元の生成メモ、Defer理由、画像上のメモを「参照のみ」として確認できます。新しい判断にはコピーされません。どちらの場合も元のRefと判断は変更せず、確認後に元のheadが変われば作成を拒否します。固定した元の記録を読めないときは、画面がその旨を表示します。

通常のarchive export／restoreはこの関係を保持します。公開形式v1と公開用文章フォームはこのprivateな来歴を表せないため、再レビューしたセッション、またはそれを含む全件exportを拒否します。今回確認したpre-reuse v0.8.1 binaryではarchive restore、fsck、`creator-report`は成功しますが、`creator-report`の出力にはこの関係を表示しません。この版の`creator-report`とlocalhost applicationで固定した来歴を検証・表示してください。古いbinaryは再利用した履歴に対する公開形式v1の拒否も保証しないため、公開前には現在のtoolを使います。
