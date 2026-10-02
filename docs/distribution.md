# SynapseGit GitHub配布ガイド

Audience: maintainer、release担当、公開文書を更新するcontributor
Status: release運用runbook
Applies to: v1.0.0 release preparation
Last verified: 2026-10-02

この文書は、SynapseGitを「GitHub上で見つける」「現在の用途を判断する」「安全に試す」までの
公開導線とrelease手順を定義する。protocolの規範仕様ではない。

## 配布の対象

現在のprimary audienceは、Linux CLIを扱えるtechnical creator、creative provenance／
human-in-the-loop AIを評価する研究者・tool builder、Rust developerである。画家、建築家、
施工・修復担当、デザイナーは将来の対象だが、capture、visual diff、restart-durable review、
production serviceが必要な一般導入にはまだ適さない。v0.3.0で導入されたlocalhost UIの
boundedな三file import、same-process Human review、read-only diagnostics、確認付きbackground
`fsck`はv0.4.0にも収録される。この範囲に限ってwrite-capable／maintenance-capableである。

v0.5.0でgeneric-artifact v1 workflow／schema／local projectionはtagged sourceのworkspace
libraryとして固定され、v0.5.1、v0.6.0、v0.7.0、v0.8.0、v0.8.1、v0.9.0、v0.10.0、v0.11.0、v0.11.1、v0.12.0、v0.13.0、v0.13.1にも引き継がれるが、release archiveの
利用者向けsurfaceには追加しない。generic-artifact用のHTTP／CLI／browser UI、新binary、remote
publish adapterは提供しない。

v0.6.0はlocalhost UIへbounded read-only archive listing（`GET /archives`、任意の
`--archive-root`起動flag指定時のみ）を追加した。v0.7.0は同じflag下で認証付きbounded no-replace
archive export API（`POST /archive-exports`）とbounded empty-target archive restore API
（`POST /archive-restores`）を追加した。v0.8.0はそのproject-page browser controls、import
preflight、manual image comparison、decision review、Creator private notes／derived candidates／public-text
formを三binaryに収録する。既存三binary構成に変更はない。
v0.9.0はInboxからの明示的な取り込み、完了Defer／中断Proposalの別sessionでの再レビュー、
boundedなproject概要と画像overlayを追加する。既存repository向けコマンドは誤ったpathにlayoutを作らない。
新規作成は`synapse init`／`creator-run`または既存の空directoryを登録した`synapse-local`で行える。
Core object／OID／archive formatは変更しない。
v0.10.0から、archiveは壁画tutorial runner(`scripts/run_mural_tutorial.sh`)、
その3枚のsynthetic sample画像(`docs/tutorial/assets/`)、tag固定linkを含む同梱guide
(archive rootの`TUTORIAL.md`。tracked sourceは`scripts/ARCHIVE_TUTORIAL.md`)も含む。
展開したarchiveだけでtutorialを実行できるようにするためで、三binary構成自体には変更が
ない。v0.9.0 archiveにはこれらは含まれない。v0.10.0 archiveには含まれる。

v0.11.0から`creator-run --generation-note-file`でprivate・user-declaredな生成メモを記録できる。
このfile入力はmodel実行や作者性を証明せず、通常archiveには含まれるがpublic bundleには含まれない。英語の
focused documentation pathと、実制作Pilot用の日本語チェックリスト／振り返りテンプレートもtagged sourceに含む。
CLIで取り込みと後日の判断を分ける経路、Git importer、GitHub App、remote publish、hosted serviceは追加しない。

v0.13.1はcomplete Creator sessionで、既存の認証付きsession detailを操作時に改めて検証して読み、
`{"state":"complete","report":{...}}`を非公開JSONとして保存できる。pending／incompleteには表示せず、
再取得した記録の検証に失敗した場合は保存しない。理由、`generation_note`、annotation／pin、internal ID、
source lineageを含み得る。このfileはpublic bundleでもrepository backupでもなく、CLIの
`creator-report --format json` documentと交換可能な形式ではない。Core、OID、archive、API revision、
CLI contract、public bundle formatを変更しない。

v0.13.1では、project dashboardの最大200件の未検証概要を維持したまま、一覧外のsessionを
**名前でセッションを開く**から完全一致の名前で直接開ける。名前は大文字小文字を区別する1〜64文字の
strict slugで、小文字から始まり小文字英数字とハイフンだけを使う。検索や一覧の拡張ではなく、存在しない
名前は既存のsession-not-found errorになる。操作にはJavaScriptが必要で、無効時も一覧は読めるが名前で
古いsessionを開くことはできない。

公開文面では、将来の利用構想とv1.0.0で実行できる能力を同じものとして表示しない。

## 公開surface

| Surface | 役割 | 正本 |
|---|---|---|
| GitHub About | 検索結果で用途を伝える | この文書のmetadata節 |
| Root README | 60秒で対象・価値・試し方・限界を判断する | [`README.md`](../README.md) |
| 日本語README | 日本語利用者の同等入口 | [`README.ja.md`](../README.ja.md) |
| GitHub Release | 固定versionのbinary、checksum、release notes | `docs/releases/vX.Y.Z.md`とtag workflow |
| Local PublicationBundle | 作者外の人／AIが読むderived JSON、Markdown、static HTML | `synapse-publication`のcanonical projectionとlocal generator |
| Documentation index | 評価・実装・運用資料を探す | [`docs/README.md`](./README.md) |
| Security / Support | 非公開報告と通常問い合わせを分離する | [`SECURITY.md`](../SECURITY.md)、[`SUPPORT.md`](../SUPPORT.md) |
| Issues / Pull requests | 再現可能なfeedbackと変更を受ける | `.github` templates |

Stage 0ではcrates.io、GHCR、Homebrew、OS package repositoryを配布channelにしない。現行
Dockerfileはprivate GCP Core CLI smoke専用であり、end-user imageとして紹介しない。Docker imageを
publication bundleやgeneric-artifact workflow、remote publishの配布経路にしない。

## GitHub About metadata

推奨description:

> Git-like, local-first Rust CLI and viewer for creative-work provenance: files, observations, AI proposals, evidence, and human decisions.

推奨topics:

```text
rust
cli
local-first
content-addressable-storage
data-provenance
creative-tools
human-in-the-loop
digital-preservation
json-schema
sqlite
```

`git`、`image-diff`、`cloud-service`は、互換性または未実装機能を誤認させるため現時点では
付けない。Topicsは機能追加時に増やすのではなく、公開利用者が実際に辿れる用途に合わせる。

外部project siteができるまではWebsite欄を空のままにする。通常releaseはGitHubの
`/releases/latest`対象になるが、install commandにはversion固定URLを使う。

## Social Preview

repository内の候補画像は[`docs/assets/social-preview.png`](./assets/social-preview.png)とする。
GitHub SettingsのSocial previewへ明示的にuploadしない限り、repositoryへ置くだけでは反映されない。

公開前に次を確認する。

- 1280 × 640相当の2:1 landscapeで、1 MB未満
- `SynapseGit`、短い価値提案、`v1.0.0`、AI提案→人の判断→local archiveの流れを簡潔に示す
- 実装済みUIのように見える架空画面を使わない
- mobile share cardでも名称が読める
- dark/light backgroundの両方で主要文字が読める

2026-10-02版は現行のbuilt-in `image_gen`で更新したconcept graphicです。
生成promptとhashは[`assets/image-generation.json`](./assets/image-generation.json)に記録します。
GitHub Settingsへのuploadは、このrepositoryの画像更新とは別の操作です。

## Release channelと対応platform

| Channel | Support | Notes |
|---|---|---|
| Linux x86_64 GNU archive | Supported release path | Ubuntu 22.04 build、glibc 2.34+ |
| macOS arm64（Apple Silicon）archive | Supported release path | macOS 14でbuild・test・archive smoke。Appleの署名・notarizationなし |
| Tagged source build | Best-effort | Rust 1.88+、対応Unix-like host |
| Windows | Unsupported | atomic archive publication path未対応 |
| Linux ARM64 prebuilt | Not published | release pipelineで未検証。tagged source buildを使う |
| Public cloud / container service | Not published | architectureまたはprivate smokeのみ |

新しいplatformは、buildが通るだけで配布対象にしない。tag workflowでtest、binary smoke、archive
展開後smokeを実行でき、security boundaryと制限をrelease notesへ記述してから追加する。

## Release asset構成

tag workflowは、Linux x86_64（`ubuntu-22.04`）とmacOS arm64（`macos-14`）のjobで、それぞれtest、release build、
archive作成、展開後smoke（`scripts/smoke_release_archive.sh`）、build provenance attestationを行う。publish jobは
両方のarchiveを集め、`scripts/assemble_release_assets.sh`で2行の`SHA256SUMS`にまとめてからdraft releaseへuploadする。
利用者は`sha256sum --check --ignore-missing SHA256SUMS`（macOSでは該当行を`shasum -a 256 --check`へ渡す）で、
downloadしたarchiveだけを検証する。main／Pull RequestのCIも、同じmacOS jobと両archiveの組み立てを毎回検証する。

v1.0.0 archiveは、v0.11.1、v0.10.0と同じ`synapse`、`synapse-local`、`synapse-present`の三binaryだけを含む。
generic-artifact v1のworkflow／schema／local projectionはtagged sourceに含まれるworkspace libraryであり、
archiveへ第四のbinaryや既存binaryのgeneric HTTP／CLI／UI surfaceを追加しない。
公開済みv0.6.0 archiveも同じ三binary構成であり、後から内容を変更しない。
公開済みv0.5.1 archiveも同じ三binary構成であり、後から内容を変更しない。
公開済みv0.5.0 archiveも同じ三binary構成であり、後から内容を変更しない。
v0.3.0 archiveは`synapse-present`を初めて追加した三binary構成であり、後から内容を変更しない。
公開済みv0.2.0 archiveは`synapse`と`synapse-local`の二binaryだけを含み、後から内容を変更しない。
公開済みv0.1.0 archiveは`SECURITY.md`と`CHANGELOG.md`追加前に作られたため、binary二つと
release notesの`README.md`だけを含む。

```text
synapsegit-vX.Y.Z-TARGET/
  synapse
  synapse-local
  synapse-present
  README.md
  SECURITY.md
  CHANGELOG.md
  LICENSE
  THIRD_PARTY_NOTICES.md
  TUTORIAL.md
  scripts/
    run_mural_tutorial.sh
  docs/
    tutorial/
      assets/
        mural-original.png
        mural-current.png
        mural-ai-proposal.png
```

`TUTORIAL.md`、`scripts/`、`docs/tutorial/assets/`はv0.10.0から追加された。
それ以前のrelease archiveにはこれらのpathは存在しない。`TUTORIAL.md`はarchive rootに
置き、`README.md`(release notes)の隣で見つけやすくしている。tracked sourceは
`scripts/ARCHIVE_TUTORIAL.md`で、package時にtagを埋め込んでarchive rootへcopyする。

Releaseにはarchive、全archiveを列挙した`SHA256SUMS`、tag-pinned release notesを置く。
更新後のworkflowで作るrelease archiveにはGitHub artifact attestationを生成する。checksumは
同じRelease上のbyteとの一致、attestationはGitHub Actions buildとの来歴を確認するもので、
softwareが安全であることやownerの法的意思を代替しない。

## Release gate

tagをpushする前に、次を満たす。

1. root `LICENSE`、Cargo `license-file` metadata、README、archiveの条件が一致し、license verifierを通る。
2. 全crate version、`docs/releases/vX.Y.Z.md`、`CHANGELOG.md`、`SECURITY.md`のSupported versions表を更新する。
   release notesはGitHub Release本文とarchive同梱の`README.md`を兼ねる。v0.13.2以降は、検証に失敗した
   archiveを展開・installしない原則、更新前に旧binaryでexportする注意、licenseの要点（OSI承認ではないこと、
   別途の書面許諾、`THIRD_PARTY_NOTICES.md`）、tag固定のinstallation guide linkを含め、公開前を前提にした
   文面を残さない。`node scripts/verify_docs.mjs`がこれらとSupported versions表の版数を検査する。
3. root READMEと日本語READMEのversion、platform、boundaryを更新する。
4. `docs/project_status.md`とcapability tableを更新する。
5. 次の検証をclean checkoutで実行する。v0.11.0以降は、Bash fenced block、workspace direct-dependency
   diagram、OpenAPI revision registry、publication browser checksもこのgateに含まれる。

```bash
cargo fmt --all -- --check
cargo test --workspace --all-targets --locked
cargo test --workspace --doc --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked
for script in scripts/*.mjs; do node --check "$script"; done
bash -n scripts/*.sh
node scripts/verify_byte_identity_allowlist.mjs --self-test
node scripts/verify_byte_identity_allowlist.mjs
node scripts/verify_core_fixtures.mjs
node scripts/verify_local_api.mjs
node scripts/test_local_api_version.mjs
node scripts/test_local_app.mjs
node scripts/test_publication_comprehension_scorer.mjs
node scripts/verify_license.mjs
node scripts/generate_third_party_notices.mjs --check
node scripts/verify_docs.mjs
node scripts/test_verify_docs.mjs
node scripts/verify_workspace_diagrams.mjs
node scripts/test_verify_workspace_diagrams.mjs
node scripts/verify_mermaid.mjs
node scripts/manage_github_security.mjs --validate
node scripts/wait_for_main_ci.mjs --self-test
node scripts/test_release_version.mjs
node scripts/test_verify_release_attestation.mjs
node scripts/verify_release_fixtures.mjs --self-test
node scripts/verify_release_fixtures.mjs
git diff --check
cargo build --release -p synapse-cli -p synapse-local-http --locked
bash scripts/verify_archive_compatibility.sh target/release/synapse
npm ci --prefix scripts/browser --ignore-scripts
scripts/browser/node_modules/.bin/playwright install --with-deps chromium
SYNAPSEGIT_BROWSER_PROFILE=release npm --prefix scripts/browser test
```

`verify_archive_compatibility.sh` first checks that the supplied binary matches
the current `synapse-cli` version. It always tests the pinned v0.11.1 source
baseline and also tests the latest eligible local annotated ancestor release
below that version when distinct. It preserves old native JSON fields while
allowing additive current fields, then verifies both archive round trips. The
read-compatibility promise itself is checked by the release fixtures below and
described in the [compatibility policy](./compatibility.md). Local
runs default to `CARGO_NET_OFFLINE=true` and require cached old-source
dependencies. Controlled CI or release jobs can set `CARGO_NET_OFFLINE=false`;
the archived old Cargo build may then download dependencies. Local `git archive`
selection itself does not fetch or download commits.

`verify_release_fixtures.mjs` requires a repository and archive fixture, written by
the release's own binary, for every annotated release tag below the current version.
After each release, download and verify its archive and run
`scripts/generate_release_fixture.sh` (see
`crates/synapse-cli/tests/fixtures/releases/README.md`) before the next version bump.

`verify_byte_identity_allowlist.mjs` recomputes the byte-identity implementation
OID of every listed release tag from its tagged source and requires every local
annotated release tag below the current version. After a version bump, add the
entry it prints for the previous release to
`crates/synapse-creator/src/report.rs`. It needs the release tags locally, as
the archive gate does.

browser testはChromiumとbrowser dependencyを必要とする。詳細と一時成果物の扱いは
[browser regression tests](../CONTRIBUTING.md#browser-regression-tests)を参照する。tag workflow自体は
release assetを作成する。上記browser checksは`main`／Pull Request CIで実行し、tag前に同じ
clean checkoutで実行する。このgateは、main CIとtag workflowが検証・packageするものと同じrelease profileの
binaryを使う。開発時のlocal既定がdebug profileであることは変わらない。

6. version commit（squash merge commit）に対する`main`のCIが、browser suite、archive互換gate、packaging検証を
   含めて成功したことを確認してから、そのcommitを指すannotated tagを作る。`main`へのpushのCIは、後続の
   mergeでcancelされない。`gh pr checks --watch`はpush直後に即終了することがあるため、
   `gh run list --workflow CI --branch main --commit <merge-commit>`でrun IDを調べ、
   `gh run watch <run-id> --exit-status`で完了を待つ。tag workflowも、build前に`scripts/wait_for_main_ci.mjs`で
   同じcommitのmain CI成功を待つ。失敗・cancel・未実行のままなら公開しない。その場合は、該当するmain CI runを
   成功までre-runしてから、release workflowをre-runする。署名運用を導入した後はsigned tagを必須にする。
7. tag workflowが通常のGitHub Releaseを作り、asset upload、checksum、attestation、公開まで成功したことを確認する。
8. 別directoryへassetをdownloadし、checksum、attestation、三binaryの`--version`／`--help`、3-file Pilot、
   read-only local publication bundleのexport／previewを確認する。archiveに第四のbinaryや
   generic-artifact HTTP／CLI／UI surfaceが紛れ込んでいないことも確認する。

   `gh attestation verify`はnon-TTY実行時（パイプ経由・スクリプト内実行時など）、plain formatの
   human-readable出力を表示しない（v0.5.1 releaseの検証で実測）。exit code 0だけでは確認内容が
   残らないため、script実行や別directoryでの検証では`--format json`を付け、JSON出力から次の3点を
   確認する。

   - workflow identityが`https://github.com/howlrs/synapsegit/.github/workflows/release.yml@refs/tags/<tag>`で
     あること（repository識別部を含む完全一致で確認する。suffixのみの確認はforkの同名workflowも通してしまう）
   - 対象artifactのdigestが`SHA256SUMS`の値と一致すること
   - runnerが`github-hosted`であること

   ```bash
   gh attestation verify <archive> \
     --repo howlrs/synapsegit \
     --signer-workflow howlrs/synapsegit/.github/workflows/release.yml \
     --source-ref refs/tags/<tag> \
     --deny-self-hosted-runners \
     --format json
   ```

   workflow identityとrunnerは`verificationResult.signature.certificate`配下の`buildSignerURI`／
   `runnerEnvironment`で、digestは`verificationResult.statement.subject[].digest.sha256`で確認できる
   （gh CLI 2.78.0での実測。fieldの厳密なpathはgh CLIのversionで変わりうるため、あくまで参考とする）。

   `GET /api/v1/archives`をsmoke testするときは、health endpointと異なり
   `X-Synapse-Local-Token`が必須である。起動済みdashboardのHTMLからprocess-local tokenを取得し、
   headerで送信する。tokenをURL、log、process argumentへ残さない。

   ```bash
   (
     SYNAPSE_LOCAL_ORIGIN=http://127.0.0.1:8787
     SYNAPSE_LOCAL_TOKEN="$(
       curl --fail --silent --show-error "$SYNAPSE_LOCAL_ORIGIN/" \
         | sed -n 's/.*<meta name="synapse-local-token" content="\([^"]*\)".*/\1/p'
     )"
     test -n "$SYNAPSE_LOCAL_TOKEN" || exit 1
     printf '%s\n' \
       "header = \"X-Synapse-Local-Token: $SYNAPSE_LOCAL_TOKEN\"" \
       "url = \"$SYNAPSE_LOCAL_ORIGIN/api/v1/archives\"" \
       | curl --fail --silent --show-error --config -
   )
   ```

### Release candidate

v1.0の前に配布するrelease candidateは、`vX.Y.Z-rc.N`（Nは1以上で先頭0なし。例: `v1.0.0-rc.1`）のannotated tagで
公開する。手順は通常のrelease gateと同じで、次だけが異なる。

- 全crateの`version`を`X.Y.Z-rc.N`にし、release notesを`docs/releases/vX.Y.Z-rc.N.md`に置く。
  `verify_release_version.sh`はtag、crate version、release notesの一致を確認する。release notesの必須文検査
  （`verify_docs.mjs`）とtag固定のinstall guide linkも、rcのtag名で適用する。
- `SECURITY.md`のsupported versionsは、rcの`X.Y`に対応する`Latest vX.Y.x prerelease`とする。
- versionの順序はSemVerに従い、`v1.0.0-rc.1` < `v1.0.0-rc.2` < `v1.0.0`である。
- archive互換性gateの基準（`select_archive_compat_baseline.mjs`）には通常のreleaseだけを使い、rcは基準にしない。
- 公開済みrcのbinaryで作ったsessionを後の版で読めるように、`verify_byte_identity_allowlist.mjs`はrcのtagも
  allowlistの登録対象にする。
- tag workflowはrcをdraft prereleaseとして作り、titleを`SynapseGit vX.Y.Z-rc.N — release candidate`にする。
  checksum、attestation、smokeは通常のreleaseと同じである。

v1.0.0はprereleaseではない通常のreleaseとして公開し、GitHub Releaseのlatestとして扱う。
`vX.Y.Z-rc.N`は引き続きcandidate向けのprereleaseである。

`v1.0.0-rc.1`はこの手順で公開する最初のcandidateであり、Linux x86_64に加えてmacOS arm64
archiveを初めて含める。これはv1.0.0の正式版ではなく、[#192](https://github.com/howlrs/synapsegit/issues/192)
で主対象の実利用者3〜5人が評価キットv2で試用するためのprereleaseである。正式版は、
[v1.0 release plan](./v1_release_plan.md#リリース条件)のPilotと他の条件を満たした後に判断する。

## 公開後check

`post-publish-verify` jobは、公開済みReleaseから両archiveと`SHA256SUMS`を別directoryへdownloadし、
Linux x86_64とmacOS arm64のrunnerで検証する。checksumの2行とarchive名、attestationの
workflow／tag／tagged commit／`github-hosted` runner／digestを確認した後、展開したbinaryのversion、help、
3-file Pilot、`inbox put`、local publication、tutorialを検査する。失敗時はworkflowが失敗となり、
公開済みRCの検証が完了したと扱わない。

- Release URLをsign-out状態で開ける
- READMEのversion固定download URLが200を返す
- archive内READMEとonline release notesのboundaryが一致する
- `SHA256SUMS`が全archiveを過不足なく列挙する
- GitHub Actionsのtag runが対象commitをbuildしている
- About description/topicsとSocial Previewが反映される
- Security Advisoriesに`Report a vulnerability`が表示される
- Community ProfileでREADME、CONTRIBUTING、Issue template、PR templateを検出する
- `docs/usage_guide.md`やpresentation guideにprivate repository等の古い状態が残っていない

## License policy

Copyright holderはhowlrsとK-Terashimaである。SynapseGitには独自の
[`SynapseGit Source-Available License 1.0`](../LICENSE)を適用し、OSI承認のopen-source
licenseとして表示しない。

許可する範囲は、GitHubでの閲覧とFork、Fork内のsource改変、GitHub-hosted CI、upstreamへの
Pull Request、および非商用評価またはFork／PR準備のための管理下環境でのclone、build、実行、
testである。commercial／production／hosted利用、GitHub Fork以外でのsource・binary再配布、
Release／Package／container／mirrorの公開には別途書面の許可を必要とする。正確な定義と条件は
root `LICENSE`を正本とする。元のarchiveへ`LICENSE`を収録していないv0.1.0も適用対象である。

独自licenseに架空のSPDX identifierを割り当てない。Cargoは
`[workspace.package] license-file = "LICENSE"`と各crateのinheritanceを使う。GitHubの
license detectorが`Other`または未検出と表示しても、OSI license名へ置き換えない。

license変更時は少なくとも次を同じPull Requestで更新する。

- root `LICENSE`と日本語概要
- `Cargo.lock`から生成した`THIRD_PARTY_NOTICES.md`
- Cargo `license-file` metadataと全crateのinheritance
- release archiveの`LICENSE`
- README、install guide、release notes、contribution条件
- `scripts/verify_license.mjs`の期待値

## 関連資料

- [Installation](./install.md)
- [Project status](./project_status.md)
- [Release notes](./releases/v1.0.0.md)
- [Security model](./security_model.md)
- [Contributing](../CONTRIBUTING.md)
- [Documentation index](./README.md)

v0.8.0 introduced a new Creator candidate from any verified complete session in the same project. It reuses exact Original/Current bytes, keeps fresh identities and Human review, and records fixed source lineage across archive/restore. Adopt does not promote the old AI output to Current. Frozen publication v1 refuses derived sessions because it cannot represent reused reference images. See the [reused source contract](../spec/application/creator-source/v1/README.md).
