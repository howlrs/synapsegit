# 公開用の制作ノートを作る

English: [Public-text workflow](./presentation_sidecar.en.md).

[![手元の制作メモとは別に、タイトル・作品の説明・制作者表記を公開用の文章として書く](./assets/private-public-notes.ja.png)](./assets/private-public-notes.ja.png)

_公開用の文章を用意する説明イラストです。文章を書き出しても、画像や制作メモがそのまま共有されることはありません。_

このフォームはv0.8.0で導入され、current v1.2.0 release binaryにも含まれます。
localhostのプロジェクト画面で「公開用の制作ノートを作る」を開き、通常の3画像取り込みで
作成した完了セッションを一つ選びます。参照画像を再利用した派生セッションは公開形式v1に未対応で、
フォームの確認時に拒否されます。作品タイトル、概要、公開用表示名、セッションのタイトル、
3画像のcaption、公開用の判断メモを入力できます。すべて任意です。セッションを選ぶと、
作品タイトルと公開用表示名に、そのセッションに記録した件名と制作者名を候補として入れます
（300 byteを超える件名は入れません）。候補は編集・削除でき、自分で編集した値は上書きしません。
候補が入った時は「現在表示中の値を確認」のcheckboxを付けるまで確認・書き出しができず、
タイトルか表示名を編集すると確認が外れます。その他の欄は空欄で始まります。
保存済みのprivate rationale、プロンプト、生成メモ、画像上の判断メモは自動転記しません。

## 公開bundleに入るもの・入らないもの

bundleにはsession名、判断、選択roleの事実、Blob／Ref OIDなど相関し得るtechnical identifierが入ります。
記録した件名や制作者名、生成メモ、rationale、decision pin、internal Actor ID、repository path、raw asset bytesは
自動では入りません。作品名・制作者表記を出すには`title = "作品名"`、
`creator_display_name = "公開用表示名"`を明示します。どちらもauthor-suppliedな公開文で、記録値の検証済みcopyではありません。

「入力した文章を確認」でサーバーが既存のPresentationInput validatorを適用します。
確認画面は自分で入力した文章のみを表示し、最終bundleの検証済みプレビューではありません。
内容を編集すると前の確認は解除されます。「説明文ファイルを書き出す」で
`presentation.toml`をブラウザからダウンロードします。下書きの永続保存やTOML取り込みはありません。

上限はUTF-8 bytesで、各タイトル・表示名300、概要8192、公開用判断メモ5120、
各caption1024です。概要と判断メモは改行できます。他は1行です。既存validatorの
制御文字・方向制御文字制限を適用し、生成TOML全体は64 KiB以下に限定します。
空欄は省略され、引用符・改行・バックスラッシュはTOML serializerがescapeします。

書き出しはCore object／Refを更新せず、raw画像・thumbnailを含まず、外部通信・公開も行いません。
文章はauthor-suppliedであり、検証された制作履歴や作者性の証明にはなりません。

## 既存CLIでbundleを生成・検証する

参照画像を再利用した派生セッションは公開形式v1に未対応です。再利用したCurrentを
新しい観測として表示しないため、フォームの確認とCLIのbundle生成で拒否します。
completeな派生セッションを含むprojectの全件exportも拒否します。通常の3画像取り込みで作成した
セッションを`--session`で選択してください。既存v1 bundleの検証は継続できます。

まず`synapse serve`と同じsourceへ書くすべてのwriterを停止し、Ref SQLiteがcheckpoint済みで
あることを確認します。稼働中sourceからの生成はこのフォームの機能に含みません。
次のSOURCE・BUNDLE・SESSIONをローカルのsource、まだ存在しない出力先、選択したsession IDへ
置き換えます。

```sh
synapse present export SOURCE BUNDLE --session SESSION --presentation presentation.toml
synapse present preview BUNDLE
```

`preview`は既存bundleを検証して入口を表示するコマンドです。`read_only_source_busy`なら
writer停止・checkpointを確認してください。説明文ファイルの作成、bundle生成・検証、外部共有は
それぞれ別の操作です。詳細は[CLI reference](cli_reference.md)を参照してください。
