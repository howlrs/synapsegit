# Run the mural tutorial from this archive / このアーカイブだけで壁画チュートリアルを試す

This file ships inside the release archive at the archive root as
`TUTORIAL.md`, next to `README.md` (the release notes). The bundled runner
(`scripts/run_mural_tutorial.sh`) and the sample images
(`docs/tutorial/assets/`) are in the same extracted directory. You are
already reading this file from inside that directory, so every step below is
self-contained: it works offline, with no Rust toolchain and no network
access at run time.

このfileはrelease archiveのrootに`TUTORIAL.md`として同梱されており、この
bundleの`README.md`(release notes)の隣にあります。同梱のrunner
(`scripts/run_mural_tutorial.sh`)とsample画像(`docs/tutorial/assets/`)も同じ
展開先directoryにあります。あなたは今、まさにそのdirectoryの中でこのfileを
読んでいます。以下の手順はすべて自己完結しており、Rust toolchainも実行時の
network accessも不要です。

Archives released at v0.10.0 and later contain this file, the runner, and the
sample images. The v0.9.0 archive does not include them; use
`scripts/run_mural_tutorial.sh` from a checkout of the v0.9.0 tag instead
(`git clone --branch v0.9.0 --depth 1 ...`), or upgrade to a later release.

v0.10.0以降のarchiveにはこのfile・runner・sample画像が含まれます。v0.9.0を使う場合は
v0.9.0 tagをcheckoutしたsourceの`scripts/run_mural_tutorial.sh`を使うか、それより後の
releaseへupgradeしてください。

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
  Only `adopt` selects the proposal; `reject` and `defer` both keep the base
  state and record `selected=false`.

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
  あり、authorship・truth・rightsの主張ではありません。`adopt`だけが提案を選択
  し、`reject`と`defer`はどちらもbase stateを維持して`selected=false`を記録
  します。

## Prerequisites / 前提条件

- Linux x86_64, Bash, glibc 2.34 or newer (the same requirement as installing
  the archive).
- No Rust toolchain, no internet access, and no other SynapseGit repository at
  the target path.

- Linux x86_64、Bash、glibc 2.34以降(archive installと同じ要件)。
- Rust toolchainもinternet accessも不要です。対象pathに既存のSynapseGit
  repositoryがないことを確認してください。

## 1. Put the bundled binaries on PATH / 同梱binaryをPATHへ置く

This assumes you already verified `SHA256SUMS` and extracted the archive as
described in the [installation guide](https://github.com/howlrs/synapsegit/blob/{{RELEASE_TAG}}/docs/install.md#install-the-linux-x86-64-release).
Move into the extracted directory and install the three binaries from there:

```bash
cd synapsegit-{{RELEASE_TAG}}-x86_64-unknown-linux-gnu

mkdir -p "$HOME/.local/bin"
install -m 0755 synapse "$HOME/.local/bin/synapse"
install -m 0755 synapse-local "$HOME/.local/bin/synapse-local"
install -m 0755 synapse-present "$HOME/.local/bin/synapse-present"
export PATH="$HOME/.local/bin:$PATH"

synapse --version
synapse-local --version
synapse-present --version
```

事前に[installation guide](https://github.com/howlrs/synapsegit/blob/{{RELEASE_TAG}}/docs/install.md#install-the-linux-x86-64-release)
の手順で`SHA256SUMS`を検証しarchiveを展開済みである前提です。展開先directoryへ
移動し、そこから3つのbinaryをinstallしてください。手順は上記bash blockと同じ
です。

## 2. Run the bundled tutorial runner / 同梱のtutorial runnerを実行する

You are currently inside the extracted directory, so run the runner with a
relative path. Give it a repository path that does not exist yet.

```bash
./scripts/run_mural_tutorial.sh "$HOME/SynapseGit/mural-tutorial" adopt
```

Use `adopt`, `reject`, or `defer` as the second argument. The command prints
the Proposal and Decision Ref heads, then runs `synapse creator-report`
automatically. Its final lines print the exact `synapse-local` command for
step 4.

The runner also works from any other current directory: it resolves the
sample images relative to its own location inside the archive, not to your
working directory. For example, `/path/to/synapsegit-{{RELEASE_TAG}}-x86_64-unknown-linux-gnu/scripts/run_mural_tutorial.sh`
works the same way from anywhere.

今いる展開先directoryから、相対pathでrunnerを実行してください。まだ存在しない
repository pathを渡します。上記bash blockと同じcommandを使い、第2引数に
`adopt`・`reject`・`defer`のいずれかを指定します。実行するとProposal／Decision
Ref headが表示され、`synapse creator-report`が自動実行されます。最後の行に、
手順4で使う`synapse-local`のcommandがそのまま印字されます。

runnerは他のどのcurrent directoryから実行しても動作します。sample画像は
runner自身のarchive内の位置から解決され、実行時のworking directoryには依存
しません。例えば`/path/to/synapsegit-{{RELEASE_TAG}}-x86_64-unknown-linux-gnu/scripts/run_mural_tutorial.sh`
のように絶対pathで呼んでも同じ結果になります。

Argument checks and refusals are unchanged: a missing or misspelled decision
argument exits with status `2`; an existing repository path is refused with
status `1` and nothing is created or overwritten.

引数checkと拒否動作は変わりません。decision引数が不正・欠落している場合は
status `2`で終了し、既存のrepository pathを渡した場合はstatus `1`で拒否され、
何も作成・上書きされません。

## 3. Read the report / reportを読む

Look for these fields in the printed `creator-report` output. `disposition`
matches the decision you chose; `selected` is `true` only for `adopt` and
`false` for `reject` or `defer`. The other fields below are the same for all
three decisions:

```text
disposition=adopt
selected=true
ai_output_source=caller_supplied
comparison_comparability=partial
byte_identity=different
comparison_warning="Different Blob bytes do not establish visual or physical change."
fsck=clean
```

For `reject` or `defer`, expect `disposition=reject` or `disposition=defer`
with `selected=false`; every other field shown above stays the same.

`fsck=clean` confirms the repository passed its integrity check; it is not a
claim about the artwork.

印字された`creator-report`の出力で上記fieldを確認してください。`disposition`は
選んだdecisionと一致し、`selected`は`adopt`のときだけ`true`、`reject`・`defer`
では`false`になります。それ以外のfieldは3つのdecisionで共通です。`reject`・
`defer`では`disposition=reject`または`disposition=defer`と`selected=false`に
なり、それ以外のfieldは変わりません。`fsck=clean`はrepositoryのintegrity check
が通ったことを示すだけで、作品についての主張ではありません。

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
