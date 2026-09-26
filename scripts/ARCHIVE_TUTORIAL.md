# Run the mural tutorial from this archive / このアーカイブだけで壁画チュートリアルを試す

This file ships inside the release archive at `scripts/ARCHIVE_TUTORIAL.md`, next
to the bundled runner (`scripts/run_mural_tutorial.sh`) and the sample images
(`docs/tutorial/assets/`). It is self-contained: every step below works from a
freshly extracted archive, offline, with no Rust toolchain and no network
access at run time.

このファイルはrelease archive内の`scripts/ARCHIVE_TUTORIAL.md`に同梱されており、
同じarchive内のrunner(`scripts/run_mural_tutorial.sh`)とsample画像
(`docs/tutorial/assets/`)の隣にあります。展開したarchiveだけで完結し、Rust
toolchainも実行時のnetwork accessも不要です。

Only archives released after `v0.9.0` contain this file, the runner, and the
sample images. If you downloaded the `v0.9.0` archive, it does not include
them; use `scripts/run_mural_tutorial.sh` from a checkout of the `v0.9.0` tag
instead (`git clone --branch v0.9.0 --depth 1 ...`), or upgrade to a later
release.

`v0.9.0`のarchiveにはこのfile・runner・sample画像は含まれません。`v0.9.0`を使う
場合は`v0.9.0` tagをcheckoutしたsourceの`scripts/run_mural_tutorial.sh`を使うか、
それ以降のreleaseへupgradeしてください。

## What this tutorial does and does not show / このtutorialが示すこと・示さないこと

- The three bundled images are synthetic fixtures generated for this project.
  They are not photographs of a real artwork and are not evidence of any real
  conservation treatment.
- No AI model runs during this tutorial. The third image is recorded as a
  caller-supplied, AI-attributed proposal; SynapseGit never generates,
  executes, or verifies a model.
- The report's `byte_identity` field compares stored Blob bytes only. It can
  say whether two files are byte-for-byte identical or different; it cannot
  say whether the images look similar, whether a change is real, who made it,
  or who owns it. Treat `byte_identity=different` as "the files are not
  byte-identical," nothing more.
- The Human Decision (`adopt`, `reject`, or `defer`) is a person's recorded
  choice about the proposal, not a claim of authorship, truth, or rights.

- 同梱の3画像はこのprojectのために生成したsynthetic fixtureです。実在作品の写真
  ではなく、実際の保存修復のevidenceでもありません。
- このtutorialではAI modelを実行しません。3枚目の画像はcaller-suppliedのAI帰属
  提案として記録されるだけで、SynapseGitがmodelを生成・実行・検証することはあり
  ません。
- reportの`byte_identity`fieldは、保存されたBlob bytesの比較結果のみを示します。
  2つのfileがbyte単位で同一か異なるかは分かりますが、画像の見た目が似ているか、
  変化が実在するか、誰が行ったか、誰が権利を持つかは分かりません。
  `byte_identity=different`は「filesがbyte単位で同一ではない」以上の意味を持ち
  ません。
- Human Decision(`adopt`／`reject`／`defer`)は、提案に対する人の記録済みの選択で
  あり、authorship・truth・rightsの主張ではありません。

## Prerequisites / 前提条件

- Linux x86_64, Bash, glibc 2.34 or newer (the same requirement as installing
  the archive).
- No Rust toolchain, no internet access, and no other SynapseGit repository at
  the target path.

- Linux x86_64、Bash、glibc 2.34以降(archive installと同じ要件)。
- Rust toolchainもinternet accessも不要です。対象pathに既存のSynapseGit
  repositoryがないことを確認してください。

## 1. Extract the archive and put the binaries on PATH / archiveを展開しbinaryをPATHへ置く

Run these commands from the directory that contains the downloaded archive.
`$bundle` is the extracted directory name, for example
`synapsegit-v0.10.0-x86_64-unknown-linux-gnu`.

```bash
tar -xzf "$bundle.tar.gz"

mkdir -p "$HOME/.local/bin"
install -m 0755 "$bundle/synapse" "$HOME/.local/bin/synapse"
install -m 0755 "$bundle/synapse-local" "$HOME/.local/bin/synapse-local"
install -m 0755 "$bundle/synapse-present" "$HOME/.local/bin/synapse-present"
export PATH="$HOME/.local/bin:$PATH"

synapse --version
synapse-local --version
synapse-present --version
```

ダウンロードしたarchiveがあるdirectoryで実行してください。`$bundle`は展開した
directory名で、例えば`synapsegit-v0.10.0-x86_64-unknown-linux-gnu`です。手順は
上記bash blockと同じです。

## 2. Run the bundled tutorial runner / 同梱のtutorial runnerを実行する

The runner works from any current directory: it resolves the sample images
relative to its own location inside the archive, not to your working
directory. Give it a repository path that does not exist yet.

```bash
"$bundle/scripts/run_mural_tutorial.sh" "$HOME/SynapseGit/mural-tutorial" adopt
```

Use `adopt`, `reject`, or `defer` as the second argument. The command prints
the Proposal and Decision Ref heads, then runs `synapse creator-report`
automatically. Its final lines print the exact `synapse-local` command for
step 3.

runnerはどのdirectoryから実行しても動作します。sample画像はrunner自身のarchive内
の位置から解決され、実行時のworking directoryには依存しません。まだ存在しない
repository pathを渡してください。上記bash blockと同じcommandを使い、第2引数に
`adopt`・`reject`・`defer`のいずれかを指定します。実行するとProposal／Decision
Ref headが表示され、`synapse creator-report`が自動実行されます。最後の行に、
手順3で使う`synapse-local`のcommandがそのまま印字されます。

Argument checks and refusals are unchanged: a missing or misspelled decision
argument exits with status `2`; an existing repository path is refused with
status `1` and nothing is created or overwritten.

引数checkと拒否動作は変わりません。decision引数が不正・欠落している場合は
status `2`で終了し、既存のrepository pathを渡した場合はstatus `1`で拒否され、
何も作成・上書きされません。

## 3. Read the report / reportを読む

Look for these fields in the printed `creator-report` output (the exact value
of `disposition` matches the decision you chose):

```text
disposition=adopt
selected=true
ai_output_source=caller_supplied
comparison_comparability=partial
byte_identity=different
comparison_warning="Different Blob bytes do not establish visual or physical change."
fsck=clean
```

`fsck=clean` confirms the repository passed its integrity check; it is not a
claim about the artwork.

印字された`creator-report`の出力で上記fieldを確認してください(`disposition`は
選んだdecisionと一致します)。`fsck=clean`はrepositoryのintegrity checkが通った
ことを示すだけで、作品についての主張ではありません。

## 4. Open the localhost UI / localhost UIを開く

Run the exact command the runner printed, for example:

```bash
synapse-local --project "mural=$HOME/SynapseGit/mural-tutorial" \
  --label "mural=Community Hall Coastal Mural"
```

Open the exact `http://127.0.0.1:...` origin printed in the terminal. Press
Ctrl-C before reusing the same repository with another CLI command.

runnerが印字したcommandをそのまま実行してください(上記bash blockが例です)。
terminalに印字された`http://127.0.0.1:...`のorigin(そのまま)を開きます。同じ
repositoryへ別のCLI commandを使う前にCtrl-Cで停止してください。

## 5. Continue with your own images / 自分の画像で続ける

Pick a new, non-existing repository path and call `synapse` directly with your
own three files:

```bash
synapse init "$HOME/SynapseGit/my-project"
synapse creator-run "$HOME/SynapseGit/my-project" session-1 \
  /path/to/original.png \
  /path/to/current.png \
  /path/to/candidate.png \
  --subject "My creative work" \
  --creator "Your name" \
  --decision defer \
  --rationale "Review this candidate later."
```

新しい未使用のrepository pathを選び、自分の3fileを使って直接`synapse`を呼んで
ください(上記bash blockが例です)。

## More documentation (tag-pinned, online) / さらに詳しい文書(tag固定、online)

These links point at the exact tag this archive was built from and require
network access; nothing above depends on them.

- [Installation guide](https://github.com/howlrs/synapsegit/blob/{{RELEASE_TAG}}/docs/install.md)
- [Illustrated 15-minute mural tutorial](https://github.com/howlrs/synapsegit/blob/{{RELEASE_TAG}}/docs/tutorial/README.md)
  ([日本語](https://github.com/howlrs/synapsegit/blob/{{RELEASE_TAG}}/docs/tutorial/README.ja.md))
- [Security model](https://github.com/howlrs/synapsegit/blob/{{RELEASE_TAG}}/docs/security_model.md)
- [CLI reference](https://github.com/howlrs/synapsegit/blob/{{RELEASE_TAG}}/docs/cli_reference.md)
- [Main README](https://github.com/howlrs/synapsegit/blob/{{RELEASE_TAG}}/README.md)
  ([日本語](https://github.com/howlrs/synapsegit/blob/{{RELEASE_TAG}}/README.ja.md))

上記linkは、このarchiveがbuildされた実際のtagを指しており、network accessが
必要です。ここまでの手順はどのlinkにも依存しません。
