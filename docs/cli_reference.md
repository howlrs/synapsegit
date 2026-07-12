# SynapseGit Core CLI reference

`synapse` is a local Stage 0 interface for object ingestion, Ref updates, integrity checks, and directory archive round trips.
It is not yet a creator-facing application, network client, or authorization boundary. The Rust
workspace provides process-local authenticated AI and admitted-proposal-bound narrow Human routes
in `synapse-application`, plus `synapse-core::CreativeAiRuntime` and `HumanDecisionRuntime`, but no
current CLI command exposes either application or Core admission route.
The workspace also provides `synapse-projection::SqliteProjectionStore`, but the CLI has no
projection rebuild or query command.

Status: **implemented at Core v0.1 / Stage 0 draft**

## Build and help

```bash
cargo build -p synapse-cli --locked
target/debug/synapse --help
```

`cargo run` 経由では `--` を挟む。

```bash
cargo run -p synapse-cli -- --help
```

成功は exit code 0、全 error は現在 exit code 1。error は stderr の先頭に `<code>:` を付ける。
usage error の場合は usage 全文も stderr に出す。

## Repository layout

`init` または任意の repository command は、指定 root の下に次を作る。

```text
<repo>/
├── cas/
│   ├── objects/    immutable objects grouped by family and digest prefix
│   └── tmp/        publication staging files
└── refs.sqlite3    mutable Refs and append-only reflog
```

layout は local implementation detail である。`cas/objects` や SQLite を直接編集せず、Core API / CLI を使う。
ProjectionStoreはrepository layoutの正本ではなく、callerが別path／connectionへ明示的に構築する
disposable derived indexである。

## Commands

### `init <repo>`

repository directory、filesystem ObjectStore、SQLite RefStore を作成または開く。

```text
initialized <repo>
```

既存 repository の data は消去しない。

### `put-blob <repo> <file> [--claimed <oid>]`

file の raw bytes を streaming ingest し、Blob OID を出力する。

```bash
synapse put-blob .synapse image.png
synapse put-blob .synapse image.png \
  --claimed blob:sg-oid-v1:sha256:<64-lowercase-hex>
```

`--claimed` がある場合は再計算 OID と exact match しなければならない。
media type、filename、EXIF、access policy は Blob OID に追加されない。

### `put-record <repo> <file> [--claimed <oid>]`

JSON を strict parse、concrete Record schema、local semantic rule で検証し、canonical Record を保存する。
body の `object_type` は `record` でなければならない。

### `build-tree <repo> <file> [--claimed <oid>]`

既成 JSON body を ManifestTree family として validate + put する。

> この command は directory を走査して Tree JSON を自動生成しない。

### `commit <repo> <file> [--claimed <oid>]`

既成 JSON body を Commit family として validate + put する。

> この command は parent、author、timestamp、snapshot を自動生成しない。

### `put-object <repo> <file> [--claimed <oid>]`

body の `object_type` に従い、Record、ManifestTree、Commit を自動 dispatch する。
Blob は JSON ではないため `put-blob` を使う。

全 put command は成功時に OID だけを stdout へ出す。同じ object の再投入は同じ OID を返すが、
CLI は `Created` / `AlreadyPresent` の disposition を表示しない。

### `update-ref <repo> <ref> <expected-oid|-> <new-oid> [options]`

candidate Commit の typed closure を確認し、Ref と reflog を compare-and-swap 更新する。

```bash
synapse update-ref .synapse proposal/agent/run-1 - \"$NEW_HEAD\" \
  --actor actor:local-user \
  --message \"first proposal\"

synapse update-ref .synapse proposal/agent/run-1 \"$OLD_HEAD\" \"$NEW_HEAD\"
```

- `-` は current Ref が存在しないことを期待する create operation。
- update は current head の exact Commit OID を expected value にする。
- `--actor` と `--message` は各一回、順不同で指定できる。
- 成功時は `<ref><TAB><new-oid>` を stdout へ出す。
- closure failure または `ref_conflict` では Ref と reflog を変更しない。

allowed top-level namespace:

- `proposal/*`
- `decision/*`
- `release/*`
- `observed/*`
- `material-events/*`

これはRef nameのsyntax validationでありauthorizationではない。`--actor`の本人性も検証しない。
`update-ref`はlocal trusted operator向けの低水準`Repository::update_ref` primitiveであり、
Actor／Grant／Policy／runtime capability、project binding、Human Gate、ContextPack baseを検査しない。
AI用のcheckpoint／single-base-parent、snapshot output delta、transaction-time Grant expiryも強制しない。
AI callerには公開せず、`synapse-application`のone-shot routeを使用する。applicationは認証後のexact
project map／ACL、trusted profile／one-time current-head registration、Core `preflight_proposal`、opaque permit、
trusted Executor、実行後reauth／FIFO fenceを経て`publish_preflighted`を呼ぶ。
Human DecisionもCLIではなく同じapplicationを使用する。成功したAI receiptのsame-instance admitted handle、
reusable Human profile、server-fixed candidateからone-time registration／permitを作り、authentication／exact
project ACL／FIFO fenceを通して`HumanDecisionRuntime::publish_decision`を呼ぶ。handleはCore denial後の修正版へ
再利用できるが、registrationとpermitはone-shotである。

### `refs <repo>`

current Refs を name 順に出力する。

```text
<name><TAB><commit-oid>
```

reflog を表示する CLI command はまだない。

### `fsck <repo>`

stored object の pathname / OID / canonical bytes と、current Ref heads からの closure を検査する。

```text
objects=<seen> verified=<verified> closures=<roots> issues=<count>
```

issue detail は stderr に Rust Debug 形式で出し、問題があれば最後に `fsck_failed` で終了する。
Ref が0件なら stored Commit 全件を root とする。

`fsck` は全 structured object の concrete JSON Schema を再実行しない。
Core ingest / restore を通った object は投入時に schema 検証済みである。

### `export <repo> <archive-dir>`

Refs / reflog を snapshot し、ObjectStore 全体を checksum 付き directory archive へ export する。

- destination は存在してはならない。
- reachable object だけでなく、stored orphan object も含む。
- current Ref heads の closure が不完全なら拒否する。
- archive は暗号化・署名されない。
- 成功時は `exported <archive-dir>` を出力する。

format detail は [Local directory archive profile](../spec/core/v0.1/archive-profile.md) を参照する。

### `restore <archive-dir> <repo>`

directory archive を検証し、object、reflog、Refs を復元する。

- Ref / reflog が既に存在する repository は拒否する。
- CAS は空、または同じ manifest OID 集合の exact subset なら失敗 restore を再開できる。
- unrelated object が一つでもあれば `archive_not_empty`。
- object、checksum、OID、schema、closure を再検証する。
- Ref / reflog は object phase と closure 検証の後に公開する。
- 成功時は `restored <repo>` を出力する。

## Ref name constraints

- 全体は最大500 bytes。
- slash 区切りの各 segment は1〜128 bytes。
- segment は ASCII lowercase letter または digit で始める。
- 以降は ASCII lowercase letter、digit、`.`、`_`、`:`、`-`。
- empty segment、`.`、`..`、上記以外の top-level namespace は拒否する。

## 既定 limit

| resource | default |
|---|---:|
| CLI structured file read | 16 MiB |
| Blob | 512 MiB |
| JSON depth | 128 |
| JSON nodes | 100,000 |
| container members / items | 50,000 |
| closure objects / edges / depth | 100,000 / 1,000,000 / 512 |

完全な limit と security boundary は [Security model](./security_model.md) を参照する。

## Error code guide

| code | 意味 |
|---|---|
| `usage_error` | CLI argument または CLI-side structured size error |
| `storage_error` | filesystem、SQLite、clock、unsupported local storage state |
| `invalid_utf8` / `bom_forbidden` | structured input encoding rejection |
| `duplicate_key` / `number_token_forbidden` / `unsafe_integer` / `lone_surrogate` | strict JSON rejection |
| `key_not_nfc` / `identifier_not_nfc` | canonical identity rule rejection |
| `set_not_sorted` / `set_duplicate` | schema-marked set rejection |
| `timestamp_invalid` / `interval_invalid` | canonical time rejection |
| `fixed_point_not_normalized` | ScaledInteger normalization rejection |
| `path_segment_invalid` | Manifest path または Ref name rejection |
| `schema_invalid` | concrete schema / local semantic / graph shape rejection |
| `reference_type_mismatch` | object family または known Record target type mismatch |
| `oid_mismatch` | claimed OID、stored byte、archive object の不一致 |
| `closure_missing` | required object が unresolved |
| `authorization_denied` | admission libraryでidentity／binding／capability／Policy／snapshot／disposition／duplicate等を拒否。現CLIにadmission routeはない |
| `human_gate_required` | AIのdecision/release直接更新、または現在のtrusted routeが満たさないPolicy gate |
| `stale_base` | Creative AI publicationでContextPack expected baseとlive base Refが不一致。Human Decisionのdecision/base競合は`ref_conflict` |
| `ref_conflict` | current target Ref、またはHuman Decisionのtrusted proposal Ref/headとexpectationが不一致 |
| `resource_limit` | Core parser / graph / Blob limit |
| `archive_invalid` | destination exists、manifest、checksum、archive graph error |
| `archive_not_empty` | restore target に unrelated data または existing Ref がある |
| `fsck_failed` | `fsck` が一件以上の issue を発見 |
| `authentication_required` | application routeのcredentialをAuthenticatorが受理しない。現CLIは返さない |
| `project_access_denied` | application routeのmalformed／unknown／forbidden project、またはproject-scoped handle/profile不一致。現CLIは返さない |
| `execution_permit_invalid` | AIまたはHuman application permitがwrong session／instance、consumed、revoked、expired、またはClock backward。現CLIは返さない |
| `execution_failed` | trusted application Executorがerrorまたはpanic。現CLIは返さない |
| `configuration_invalid` | trusted application control-plane configurationが不正。現CLIは返さない |
| `service_unavailable` | applicationのlock／counter等のoperational failure。現CLIは返さない |

上表のCore admission codeは`CreativeAiRuntime`／`HumanDecisionRuntime` library boundaryで実装されている。
現在のCLI `update-ref`はContextPackやtrusted identity／Policy／proposal authorityを受け取らないため、
admission authorizationや`stale_base`を評価せず、target Ref競合には`ref_conflict`を返す。
これらapplication codeのexact messageは順に`authentication required`、`project access denied`、
`execution permit invalid`、`execution failed`、`application configuration invalid`、
`application service unavailable`である。Core semantic errorはauthorized AI／Human permitをburnしたfinal
publicationからだけ透過される。AI preflight denialは`configuration_invalid`、そのoperational failureは
`service_unavailable`へ正規化され、Humanに別Core preflightはない。これらapplication codeはCLIからは返らない。

## 未実装 command

`read-object`、`log`、`diff`、`checkout`、`merge`、Ref delete、object delete / GC、reflog view、
JSON output、stdin input、`publish-proposal`、`publish-decision`、projection `rebuild`／timeline／
dependency／Analysis lineage／closure queryは現在の CLI にない。ProjectionのRust APIは実装済みだが、automatic refresh、
SurrealDB adapter、全8-query／benchmark比較は未実装である。
`publish-proposal`／narrow `publish-decision`相当のprocess-local application library routeが実装されたことは、
HTTP／JWT、durable permit service、一般的なHuman workflow、Projection application route、または
Human Decision／Projection CLI commandを提供したという意味ではない。
