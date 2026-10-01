const TOKEN_HEADER = "X-Synapse-Local-Token";
const TOKEN_SELECTOR = 'meta[name="synapse-local-token"]';
const API_BASE_SELECTOR = 'meta[name="synapse-api-base"]';
const ENHANCED_FORMS = new WeakSet();
const COMPLETED_ARCHIVE_RESTORES = new WeakSet();
const ENHANCED_IMAGES = new WeakSet();
const CONTROL_DISABLED_STATE = new WeakMap();
const IMAGE_ELEMENTS = new Set();
const IMAGE_REQUESTS = new Map();
const IMAGE_URLS = new Map();
const INLINE_IMAGES = new WeakSet();
const PRIVATE_REPORT_URLS = new Set();
const PRIVATE_REPORT_REQUESTS = new Set();
const ALLOWED_RASTER_TYPES = new Set(["image/png", "image/jpeg", "image/gif", "image/webp"]);
const ATTACHMENT_MEDIA_TYPE = "application/octet-stream";
const MAX_IMAGE_BYTES = 64 * 1024 * 1024;
const MAX_UPLOAD_AGGREGATE_BYTES = 3 * MAX_IMAGE_BYTES;
const CREATOR_TEXT_FIELDS = new Map([
  ["session", 64],
  ["subject_label", 500],
  ["creator_name", 300],
  ["generation_tool", 300],
  ["generation_model", 300],
  ["generation_prompt", 8192],
  ["generation_intent", 2048],
]);
const CREATOR_UPLOADS = new Map();
const CREATOR_FILE_FIELDS = new Set(["original_image", "current_image", "ai_output"]);
const UTF8_ENCODER = new TextEncoder();
const CREATOR_SESSION_SLUG = /^[a-z][a-z0-9-]{0,63}$/u;

// Interface messages keyed by stable identifiers. Each entry keeps Japanese
// and English together; `node scripts/test_local_app.mjs` checks that both
// are present, non-empty, use the same placeholders, and cover every t() key.
// User data and API problem fields are never translated.
const MESSAGE_ENTRIES = [
  ["client.tokenInvalid", "ローカルブラウザーセッションのtokenがないか、不正です。ページを再読み込みしてください。", "The local browser session token is missing or invalid. Reload the page."],
  ["client.apiBaseInvalid", "設定されたAPI baseは同一originのpathである必要があります。", "The configured API base must be a same-origin path."],
  ["client.apiUrlInvalid", "APIへのrequestは同一originのAPI base内に限られます。", "API requests must stay within the same-origin API base."],
  ["privateReport.preparing", "記録を確認しています…", "Checking private record…"],
  ["privateReport.invalid", "記録を確認できませんでした。保存は開始していません。", "The private record could not be verified. Saving did not start."],
  ["privateReport.started", "記録のダウンロードを開始しました。", "Private record download started."],
  ["error.withDetail", "{summary}（詳細: {detail}）", "{summary} (Details: {detail})"],
  ["error.generic", "要求を完了できませんでした。", "The request could not be completed."],
  ["error.http", "ローカルアプリケーションがHTTP {status}を返しました。", "The local application returned HTTP {status}."],
  ["error.cancelled", "要求を取り消しました。", "Request cancelled."],
  ["error.unexpected", "ローカルアプリケーションは要求を完了できませんでした。", "The local application could not complete the request."],
  ["error.code.project_not_found", "プロジェクトが見つかりません。", "The project was not found."],
  ["error.code.creator_session_not_found", "セッションが見つかりません。", "The session was not found."],
  ["error.code.creator_session_exists", "同じ名前のセッションが既にあります。新しい名前を指定してください。", "A session with this name already exists. Use a new name."],
  ["error.code.creator_session_incomplete", "セッションが未完了のため、この操作はできません。", "The session is incomplete, so this operation is not available."],
  ["error.code.creator_review_state_lost", "このprocessのレビュー状態が失われました。ページを再読み込みして状態を確認してください。", "This process's review state was lost. Reload the page to check the state."],
  ["error.code.creator_review_busy", "レビュー処理が混み合っています。時間をおいて再試行してください。", "Review capacity is busy. Try again later."],
  ["error.code.creator_outcome_unknown", "Decisionの結果を確認できません。再送信せず、ページを再読み込みして状態を確認してください。", "The Decision outcome is unknown. Do not resubmit; reload the page to check the state."],
  ["error.code.ref_conflict", "操作中にプロジェクトが変更されました。ページを再読み込みしてください。", "The project changed during the operation. Reload the page."],
  ["error.code.stale_base", "操作中にプロジェクトが変更されました。ページを再読み込みしてください。", "The project changed during the operation. Reload the page."],
  ["error.code.resource_limit", "ローカルの処理上限を超えました。入力やファイルを小さくしてください。", "A local processing limit was exceeded. Use smaller input or files."],
  ["error.code.local_request_denied", "ローカルの安全確認で要求が拒否されました。入力と確認内容を見直してください。", "The local safety checks rejected the request. Review the input and the confirmation."],
  ["error.code.usage_error", "入力内容を処理できません。値を確認してください。", "The input cannot be processed. Check the values."],
  ["error.code.archive_invalid", "アーカイブを検証できません。Archives一覧で状態を確認してください。", "The archive could not be verified. Check its state in the Archives list."],
  ["error.code.archive_not_empty", "復元先のプロジェクトに既存の履歴があります。", "The restore target project already has history."],
  ["error.code.fsck_failed", "整合性の確認を完了できませんでした。", "The integrity check could not be completed."],
  ["error.code.creator_report_invalid", "記録を検証できませんでした。fsckで状態を確認してください。", "The record could not be verified. Check its state with fsck."],
  ["error.code.creator_implementation_unrecognized", "このセッションは、この版が認識しないSynapseGit（新しい版や未リリースのsource build）で作成されました。作成した版、またはそれ以降の版で開いてください。", "This session was recorded by a SynapseGit build this version does not recognize, such as a newer release or an unreleased source build. Open it with the recording build or a later release."],
  ["error.code.schema_invalid", "入力が記録形式の検証に失敗しました。", "The input failed record-format validation."],
  ["error.code.storage_error", "ローカルの保存データにアクセスできませんでした。時間をおいて再試行してください。", "The local storage could not be accessed. Try again later."],
  ["error.code.service_unavailable", "この機能は現在利用できません。", "This feature is not available right now."],
  ["error.code.operation_state_lost", "メンテナンス操作の状態が失われました。結果をプロジェクト画面で確認してください。", "The maintenance operation state was lost. Check the result on the project page."],
  ["operation.fsckClean", "整合性の確認が完了しました。{count} objectsを検証し、問題はありませんでした。", "Integrity check completed cleanly after verifying {count} objects."],
  ["operation.fsckIssues", "整合性の確認が完了し、{count} 件の問題が見つかりました。", "Integrity check completed with {count} issues."],
  ["operation.fsckResultInvalid", "整合性確認の結果が不正です。", "The integrity-check result is invalid."],
  ["operation.exportResultInvalid", "アーカイブ書き出しの結果が不正です。", "The archive-export result is invalid."],
  ["operation.exported", "アーカイブ「{name}」を書き出しました。", "Archive “{name}” was exported."],
  ["operation.restoreResultInvalid", "アーカイブ復元の結果が不正です。", "The archive-restore result is invalid."],
  ["operation.restored", "アーカイブ「{name}」を復元しました。復元した履歴を利用する前に、保存元と復元先の creator-report を比較し、一致を確認して保管してください。", "Archive “{name}” was restored. Before using the restored history, compare the creator-report output of the source and the restore target, confirm that they match, and keep the result."],
  ["operation.completed", "メンテナンス操作が完了しました。", "Maintenance operation completed."],
  ["operation.receiptInvalid", "メンテナンス操作の受付情報が不正です。", "The maintenance operation receipt is invalid."],
  ["operation.statusInvalid", "メンテナンス操作の状態が不正です。", "The maintenance operation status is invalid."],
  ["operation.queued", "メンテナンス操作は待機中です…", "Maintenance operation is queued…"],
  ["operation.running", "メンテナンス操作を実行中です…", "Maintenance operation is running…"],
  ["operation.failed", "メンテナンス操作は正常に完了しませんでした。", "The maintenance operation did not complete successfully."],
  ["operation.restorePartial", "ファイルの一部だけがコピー済みの可能性があります。確認後も同じアーカイブだけを再試行してください。", "Some files may already have been copied. If you retry after checking, use only the same archive."],
  ["operation.started", "Maintenance operationを開始しました…", "Maintenance operation started…"],
  ["operation.restoreStarted", "Archive restoreを開始しました…", "Archive restore started…"],
  ["operation.restoreReceiptInvalid", "アーカイブ復元の受付情報が不正です。", "The archive restore receipt is invalid."],
  ["image.decodeFailed", "画像を表示できません。画像データが破損しているか、ブラウザーが対応していません。", "The image cannot be shown. The image data is corrupt or the browser does not support it."],
  ["image.noDownloadAction", "attachment専用の応答に専用のダウンロード操作がありません。", "The attachment-only response has no dedicated download action."],
  ["image.downloaded", "Download済みです。再取得する場合はページを再読み込みしてください。", "Downloaded. Reload the page to fetch it again."],
  ["image.attachmentOnly", "Inline表示できない形式です。検証済みraw bytesをdownloadして確認してください。", "This format cannot be shown inline. Download the verified raw bytes to check it."],
  ["image.sourceUnavailable", "画像の取得元がありません。", "Image source unavailable."],
  ["image.loading", "画像を読み込んでいます…", "Loading image…"],
  ["image.unsafeResponse", "応答はinline表示できる安全な画像でも、安全なattachmentでもありません。", "The response is neither an inline-safe raster nor a safe attachment."],
  ["image.tooLarge", "画像が表示上限の64 MiBを超えています。", "The image exceeds the 64 MiB display limit."],
  ["compare.overlayUnavailable", "重ねて表示するには、A と B のデコード後の幅と高さが一致している必要があります。並べて表示で確認できます。", "Overlay needs A and B to have the same decoded width and height. You can still compare them side by side."],
  ["compare.overlayCaption", "{a} を下、{b} を上に重ねています。共通の左上原点で表示する目視補助であり、位置合わせ・差分解析は行いません。{opacity}%", "{a} is below and {b} is overlaid on top from a shared top-left origin. This is a visual aid only; no registration or difference analysis is performed. {opacity}%"],
  ["compare.paneUnavailable", "この画像は表示できません。元のカードの状態を確認してください。", "This image cannot be shown. Check the state of its original card."],
  ["compare.ready", "表示できる画像を2枚選んで、全体表示・100%・200%で確認できます。", "Choose two viewable images to compare them fit to view, at 100%, or at 200%."],
  ["compare.notReady", "比較には表示可能な画像が2枚必要です。読み込み中・表示不可の理由は各カードで確認できます。", "Comparison needs two viewable images. Each card shows whether it is still loading or why it cannot be shown."],
  ["sessions.count", "{count} 件", "{count} sessions"],
  ["sessions.countOne", "{count} 件", "{count} session"],
  ["text.tooLong", "{limit} bytes以内に短くしてください（現在 {count} bytes）。", "Shorten this to {limit} bytes or less (currently {count} bytes)."],
  ["upload.summary", "選択済み {selected} / {total} ファイル · 合計 {size} / {limit} MiB", "{selected} / {total} files selected · total {size} / {limit} MiB"],
  ["upload.fileTooLarge", "64 MiB以内のファイルを選び直してください。", "Choose a file of 64 MiB or less."],
  ["upload.previewLoading", "取り込み前のプレビューを読み込んでいます…", "Loading the preview before import…"],
  ["upload.previewUnsupported", "この形式はプレビューできません。ファイルはそのまま取り込めます。", "This format cannot be previewed. The file can still be imported as is."],
  ["upload.previewReady", "{width} × {height} px · ローカルプレビュー", "{width} × {height} px · local preview"],
  ["upload.previewFailed", "プレビューを表示できません。ファイルの内容を確認してください。そのまま取り込むこともできます。", "The preview cannot be shown. Check the file contents. You can still import it as is."],
  ["form.fileNeedsUpload", "ファイルを含むフォームには専用のupload処理が必要です。", "File forms require the dedicated upload enhancement."],
  ["form.fieldOnce", "項目「{name}」はちょうど1回だけ指定してください。", "The field “{name}” must occur exactly once."],
  ["form.fieldTooLong", "項目「{name}」がUTF-8のbyte上限を超えています。", "The field “{name}” exceeds its UTF-8 byte limit."],
  ["form.decisionTooLarge", "理由とピンを含むDecision JSONが8 KiBを超えています。", "The decision JSON exceeds 8 KiB including rationale and pins."],
  ["form.submitterInvalid", "送信ボタンがこのフォームに属していません。", "The submit control does not belong to this form."],
  ["form.textFieldOnce", "項目「{name}」はテキストとしてちょうど1回だけ指定してください。", "The field “{name}” must occur exactly once as text."],
  ["form.fileFieldOnce", "項目「{name}」はファイルとしてちょうど1回だけ指定してください。", "The field “{name}” must occur exactly once as a file."],
  ["form.fileTooLarge", "ファイル「{name}」が64 MiBの上限を超えています。", "The file “{name}” exceeds the 64 MiB limit."],
  ["form.aggregateTooLarge", "3つのファイルの合計が192 MiBの上限を超えています。", "The three files exceed the 192 MiB aggregate limit."],
  ["form.fieldNotAllowed", "項目「{name}」はcreator uploadでは使えません。", "The field “{name}” is not allowed in a creator upload."],
  ["form.uploadFieldsInvalid", "creator uploadに不足または想定外の項目があります。", "The creator upload contains missing or unexpected fields."],
  ["form.sessionInvalid", "Sessionは小文字英数字とハイフンの有効な名前にしてください。", "The session field is not a valid lowercase slug."],
  ["form.sourceConfirmationInvalid", "派生元の確認情報が不正です。派生元のページを開き直してください。", "The source confirmation is invalid. Reopen the source page."],
  ["form.getWithFile", "GETフォームにはファイルを含められません。", "GET forms cannot contain files."],
  ["form.unknownEnhancement", "APIフォームの種類を認識できません。", "The API form enhancement is not recognized."],
  ["form.working", "処理しています…", "Working…"],
  ["form.destinationInvalid", "移動先はローカルアプリケーションのoriginである必要があります。", "The success destination must use the local application origin."],
  ["form.completed", "完了しました。", "Completed."],
  ["form.sessionDestinationInvalid", "作成したセッションの移動先が不正です。", "The creator session destination is invalid."],
  ["note.tooLarge", "生成メモ全体は16 KiB以内にしてください。", "Keep the whole generation note within 16 KiB."],
  ["restore.targetInvalid", "復元先のプロジェクトが不正です。", "The restore target project is invalid."],
  ["restore.confirmationInvalid", "アーカイブ復元の確認内容が正しくありません。アーカイブ名、復元先のproject key、既存の履歴がないことの確認を見直してください。", "The archive restore confirmation is invalid. Check the archive name, the target project key, and the empty-history confirmation."],
  ["restore.confirm", "archive “{archive}” をtarget project “{project}” へ復元します。失敗時もファイルの一部がコピー済みの可能性があり、自動再試行は行いません。続行しますか？", "This restores archive “{archive}” into target project “{project}”. Even if it fails, some files may already have been copied, and it is not retried automatically. Continue?"],
  ["restore.notStarted", "Archive restoreは開始されませんでした。", "The archive restore was not started."],
  ["export.confirm", "新しいarchive “{name}” を作成します。既存archiveは上書きされません。続行しますか？", "This creates the new archive “{name}”. Existing archives are not overwritten. Continue?"],
  ["export.notStarted", "Archive exportは開始されませんでした。", "The archive export was not started."],
  ["fsck.confirm", "Read-only fsckを開始します。大きなrepositoryでは完了まで時間がかかる場合があります。続行しますか？", "This starts a read-only fsck. It can take a while on a large repository. Continue?"],
  ["fsck.notStarted", "Integrity checkは開始されませんでした。", "The integrity check was not started."],
  ["decision.receiptInvalid", "記録済みcreator receiptが不正です。", "The committed creator receipt is invalid."],
  ["decision.committedWithoutReport", "Decisionは {head} で記録されました。完全なレポートは取得できません。下の永続receiptを確認し、保管してください。", "Decision committed at {head}. The full report is unavailable; inspect and retain the durable receipt below."],
  ["decision.thisSession", "このセッション", "this session"],
  ["decision.targetWithProject", "Project “{project}” / Creator session “{session}”", "project “{project}” / creator session “{session}”"],
  ["decision.target", "Creator session “{session}”", "creator session “{session}”"],
  ["decision.confirmPublish", "{target} に {disposition} decisionを公開します。", "Publish the {disposition} decision for {target}."],
  ["decision.finality", "Deferも含め、記録後にこのセッションの判断を変更・再開する機能はありません。", "Every disposition, including Defer, completes this session. A recorded decision cannot be changed or reopened in this session."],
  ["decision.confirmContinue", "この操作を続けますか？", "Continue with this operation?"],
  ["decision.notSent", "Decisionは送信されませんでした。", "The Decision was not sent."],
  ["pins.unreadable", "注釈を表示できません。未知の形式または不正な画像対応です。", "The pins cannot be shown. They use an unknown format or an invalid image binding."],
  ["pins.invalid", "ピンの画像・座標と、各メモが1〜200 UTF-8 bytesであることを確認してください。", "Check each pin's image and coordinates, and that each note is 1–200 UTF-8 bytes."],
  ["pins.remaining", "{label}: 残り {bytes} bytes", "{label}: {bytes} bytes left"],
  ["pins.jsonSummary", "送信JSON全体（上限8192 bytes、理由とピンを含む） · {remaining}", "Whole submitted JSON (limit 8192 bytes, including rationale and pins) · {remaining}"],
  ["pins.fixInvalid", " · ピンのメモ・座標を修正してください。", " · Fix the pin notes or coordinates."],
  ["pins.markerLabel", "ピン {number}: {note}", "Pin {number}: {note}"],
  ["pins.noteMissing", "メモ未入力", "No note yet"],
  ["pins.show", "ピン {number} · {role}", "Pin {number} · {role}"],
  ["pins.noteLabel", "ピン {number} のメモ", "Pin {number} note"],
  ["pins.axisLabel", "ピン {number} {axis}座標", "Pin {number} {axis} coordinate"],
  ["pins.delete", "ピン {number} を削除", "Delete pin {number}"],
  ["pins.status", "{label} · {width} × {height} px · 画像の向きを反映した表示", "{label} · {width} × {height} px · shown with the image orientation applied"],
  ["pins.unavailable", "この画像にはピンを作成・表示できません。読み込み中、attachment扱い、または画像のdecode失敗です。元画像の状態を確認してください。", "Pins cannot be created or shown on this image: it is still loading, is treated as an attachment, or failed to decode. Check the state of the source image."],
  ["inbox.discarding", "stagingを破棄しています…", "Discarding the staged files…"],
  ["inbox.discarded", "stagingを破棄しました。候補を選んでください。", "The staged files were discarded. Choose a candidate."],
  ["inbox.discardFailed", "stagingを破棄できませんでした。{error}", "The staged files could not be discarded. {error}"],
  ["inbox.stagedAlt", "{label} のstaging済みプレビュー", "Staged preview of {label}"],
  ["inbox.download", "画像をダウンロード", "Download image"],
  ["inbox.staging", "候補を検証してstagingしています…", "Verifying and staging the candidate…"],
  ["inbox.stageInvalid", "staging応答が不正です。", "The staging response is invalid."],
  ["inbox.staged", "staging済みbytesを確認し、必要ならメタデータを編集してください。", "Check the staged bytes and edit the metadata if needed."],
  ["inbox.listInvalid", "inbox一覧応答が不正です。", "The inbox list response is invalid."],
  ["inbox.ready", "manifestは有効です。確認時に3ファイルを検証してstagingします。", "The manifest is valid. The three files are verified and staged when you review it."],
  ["inbox.notReady", "候補を検査できません。", "The candidate cannot be inspected."],
  ["inbox.notReadyReason", "候補を検査できません。（詳細: {reason}）", "The candidate cannot be inspected. (Details: {reason})"],
  ["inbox.review", "確認する", "Review"],
  ["inbox.choose", "候補を選んでください。", "Choose a candidate."],
  ["inbox.empty", "manifestを最後に書いた候補はまだありません。", "No candidates are ready yet (the manifest must be written last)."],
  ["inbox.proposalInvalid", "proposal応答が不正です。", "The proposal response is invalid."],
  ["presentation.tooLong", "{label}: {bytes} / {limit} UTF-8 bytes。上限を超えています。", "{label}: {bytes} / {limit} UTF-8 bytes. This exceeds the limit."],
  ["presentation.validating", "文章を検証しています…", "Validating the text…"],
  ["presentation.responseInvalid", "説明文ファイルの応答が不正です。", "The description file response is invalid."],
  ["presentation.emptyLabel", "説明文", "Description"],
  ["presentation.emptyValue", "未入力。既存の省略時動作を使用します。", "Nothing entered. The existing default behavior is used."],
  ["presentation.size", "対象: {session} · presentation.toml: {bytes} / 65536 bytes", "Session: {session} · presentation.toml: {bytes} / 65536 bytes"],
  ["presentation.validated", "検証できました。内容を確認して書き出してください。", "Validated. Review the content, then export it."],
  ["presentation.downloadStarted", "説明文ファイルのダウンロードを開始しました。bundle生成・検証と外部共有はまだ行っていません。", "The description file download has started. The bundle has not been generated or verified, and nothing has been shared externally."],
];

export const SUPPORTED_LOCALES = Object.freeze(["ja", "en"]);
export const LOCAL_MESSAGES = Object.freeze({
  ja: Object.freeze(Object.fromEntries(MESSAGE_ENTRIES.map(([key, ja]) => [key, ja]))),
  en: Object.freeze(Object.fromEntries(MESSAGE_ENTRIES.map(([key, , en]) => [key, en]))),
});
export const LOCAL_MESSAGE_ENTRY_COUNT = MESSAGE_ENTRIES.length;

/**
 * The server resolves the display language (explicit selection, cookie,
 * Accept-Language, then Japanese) and renders it into `<html lang>`. The
 * script follows that value, so templates and client messages never diverge.
 */
export function uiLocale() {
  const lang = globalThis.document?.documentElement?.lang;
  return typeof lang === "string" && lang.toLowerCase() === "en" ? "en" : "ja";
}

/** Look up an interface message and replace `{name}` placeholders once. */
export function t(key, params = {}) {
  const template = LOCAL_MESSAGES[uiLocale()][key] ?? LOCAL_MESSAGES.ja[key];
  if (typeof template !== "string") return key;
  return template.replace(/\{([A-Za-z0-9_]+)\}/gu, (placeholder, name) =>
    Object.hasOwn(params, name) ? String(params[name]) : placeholder);
}

let imageComparison;
let localToken;
let apiBase;

export class SynapseApiError extends Error {
  constructor(problem, response) {
    const detail = typeof problem?.detail === "string" ? problem.detail : null;
    const title = typeof problem?.title === "string" ? problem.title : null;
    super(detail || title || `Request failed with status ${response.status}`);
    this.name = "SynapseApiError";
    this.serverDetail = detail || title;
    this.status = response.status;
    this.code = typeof problem?.code === "string" ? problem.code : "unexpected_response";
    this.problem = problem;
    this.response = response;
  }
}

function readMetaContent(selector) {
  const element = document.querySelector(selector);
  return element instanceof HTMLMetaElement ? element.content.trim() : "";
}

function getLocalToken() {
  if (localToken !== undefined) return localToken;

  const token = readMetaContent(TOKEN_SELECTOR);
  if (!token || token.length > 4096 || /[\r\n]/u.test(token)) {
    throw new Error(t("client.tokenInvalid"));
  }

  localToken = token;
  return localToken;
}

function getApiBase() {
  if (apiBase !== undefined) return apiBase;

  const configured = readMetaContent(API_BASE_SELECTOR) || "/api/v1";
  const resolved = new URL(configured, window.location.origin);
  if (resolved.origin !== window.location.origin || resolved.search || resolved.hash) {
    throw new Error(t("client.apiBaseInvalid"));
  }

  resolved.pathname = resolved.pathname.replace(/\/+$/u, "") || "/";
  apiBase = resolved;
  return apiBase;
}

function resolveApiUrl(input) {
  const value = input instanceof Request ? input.url : input;
  const resolved = new URL(value, window.location.origin);
  const base = getApiBase();
  const withinBase =
    resolved.pathname === base.pathname || resolved.pathname.startsWith(`${base.pathname}/`);

  if (resolved.origin !== window.location.origin || !withinBase || resolved.username || resolved.password) {
    throw new TypeError(t("client.apiUrlInvalid"));
  }

  resolved.hash = "";
  return resolved;
}

/**
 * Fetch a localhost API resource with the process-local browser token.
 * Redirects are rejected so the custom header can never follow a redirect.
 */
export function apiFetch(input, init = {}) {
  const request = input instanceof Request ? input : null;
  const url = resolveApiUrl(input);
  const headers = new Headers(request?.headers);

  new Headers(init.headers).forEach((value, name) => headers.set(name, value));
  headers.set(TOKEN_HEADER, getLocalToken());
  if (!headers.has("Accept")) {
    headers.set("Accept", "application/json, application/problem+json");
  }

  return window.fetch(request ? new Request(url, request) : url, {
    ...init,
    headers,
    cache: "no-store",
    credentials: "same-origin",
    mode: "same-origin",
    redirect: "error",
    referrerPolicy: "no-referrer",
  });
}

async function readJson(response) {
  const type = response.headers.get("Content-Type")?.split(";", 1)[0].trim().toLowerCase();
  if (type !== "application/json" && type !== "application/problem+json") {
    throw new SynapseApiError(null, response);
  }

  try {
    return await response.json();
  } catch {
    throw new SynapseApiError(null, response);
  }
}

/** Fetch and decode a JSON response, throwing SynapseApiError for API problems. */
export async function apiJson(input, init = {}) {
  const response = await apiFetch(input, init);
  if (response.status === 204) return null;

  const data = await readJson(response);
  if (!response.ok) throw new SynapseApiError(data, response);
  return data;
}

function validOperationAccepted(value) {
  if (!value || value.state !== "queued") return false;
  if (typeof value.operation_id !== "string" || !/^[A-Za-z0-9_-]{22,128}$/u.test(value.operation_id)) {
    return false;
  }
  return value.poll_path === `/api/v1/operations/${value.operation_id}`;
}

export function operationSuccessMessage(operation, archiveRestore) {
  if (operation?.state !== "succeeded") return null;
  if (archiveRestore && operation.kind !== "archive_restore") {
    throw new TypeError(t("operation.restoreResultInvalid"));
  }
  if (operation.kind === "fsck") {
    const result = operation.result;
    if (
      !result ||
      typeof result.clean !== "boolean" ||
      !Number.isSafeInteger(result.objects_verified) ||
      !Number.isSafeInteger(result.issue_count)
    ) {
      throw new TypeError(t("operation.fsckResultInvalid"));
    }
    return result.clean
      ? t("operation.fsckClean", { count: result.objects_verified })
      : t("operation.fsckIssues", { count: result.issue_count });
  }
  if (operation.kind === "archive_export") {
    const result = operation.result;
    if (
      !result ||
      typeof result.archive_name !== "string" ||
      !/^[a-z][a-z0-9-]{0,63}$/u.test(result.archive_name) ||
      result.result_kind !== "exported" ||
      result.report_equivalence_required !== false
    ) {
      throw new TypeError(t("operation.exportResultInvalid"));
    }
    return t("operation.exported", { name: result.archive_name });
  }
  if (operation.kind === "archive_restore") {
    const result = operation.result;
    if (
      !archiveRestore ||
      operation.project_key !== archiveRestore.projectKey ||
      !result ||
      result.archive_name !== archiveRestore.archiveName ||
      !/^[a-z][a-z0-9-]{0,63}$/u.test(result.archive_name) ||
      result.result_kind !== "restored" ||
      result.report_equivalence_required !== true
    ) {
      throw new TypeError(t("operation.restoreResultInvalid"));
    }
    return t("operation.restored", { name: result.archive_name });
  }
  return t("operation.completed");
}

async function pollOperation(form, accepted, expectedOperation) {
  if (!validOperationAccepted(accepted)) {
    throw new TypeError(t("operation.receiptInvalid"));
  }
  const pollUrl = resolveApiUrl(accepted.poll_path);
  let pollDelayMs = 250;
  for (;;) {
    const operation = await apiJson(pollUrl, { method: "GET" });
    if (operation?.operation_id !== accepted.operation_id || typeof operation.state !== "string") {
      throw new TypeError(t("operation.statusInvalid"));
    }
    if (operation.state === "queued" || operation.state === "running") {
      setStatus(form, t(operation.state === "queued" ? "operation.queued" : "operation.running"), null);
      await new Promise((resolve) => window.setTimeout(resolve, pollDelayMs));
      pollDelayMs = Math.min(pollDelayMs * 2, 2_000);
      continue;
    }
    if (operation.state === "succeeded") {
      if (
        expectedOperation &&
        (operation.kind !== expectedOperation.kind || operation.project_key !== expectedOperation.projectKey)
      ) {
        throw new TypeError(t("operation.statusInvalid"));
      }
      return operation;
    }
    if (operation.state === "failed" || operation.state === "outcome_unknown") {
      const detail = operationFailureMessage(operation.error);
      throw new TypeError(
        expectedOperation?.kind === "archive_restore"
          ? `${detail} ${t("operation.restorePartial")}`
          : detail,
      );
    }
    throw new TypeError(t("operation.statusInvalid"));
  }
}

function imageStatusElement(image) {
  const targetId = image.dataset.statusTarget;
  const target = targetId ? document.getElementById(targetId) : null;
  if (target instanceof HTMLElement) return target;

  const nearby = image.closest("figure")?.querySelector("[data-synapse-image-status]");
  if (nearby instanceof HTMLElement) return nearby;

  const created = document.createElement("p");
  created.className = "image-status";
  created.setAttribute("role", "status");
  created.setAttribute("aria-live", "polite");
  image.after(created);
  return created;
}

function setImageStatus(image, message, tone) {
  const status = imageStatusElement(image);
  status.textContent = message;
  status.hidden = message.length === 0;
  if (tone) status.dataset.tone = tone;
  else delete status.dataset.tone;
}

function imageDownloadElement(image) {
  const download = image.closest("figure")?.querySelector("a[data-synapse-image-download]");
  return download instanceof HTMLAnchorElement ? download : null;
}

function resetImageDownload(image) {
  const download = imageDownloadElement(image);
  if (!download) return;
  download.hidden = true;
  download.removeAttribute("href");
  download.onclick = null;
}

function revokeImageUrl(image) {
  INLINE_IMAGES.delete(image);
  refreshPinEditors();
  const objectUrl = IMAGE_URLS.get(image);
  if (objectUrl) {
    URL.revokeObjectURL(objectUrl);
    IMAGE_URLS.delete(image);
    if (image.getAttribute("src") === objectUrl) image.removeAttribute("src");
  }
  resetImageDownload(image);
}

function creatorImageResponseMode(type, disposition) {
  if (ALLOWED_RASTER_TYPES.has(type) && disposition === "inline") return "inline";
  if (type === ATTACHMENT_MEDIA_TYPE && /^attachment(?:\s*;|$)/u.test(disposition)) {
    return "attachment";
  }
  return null;
}

async function installInlineRaster(image, objectUrl) {
  resetImageDownload(image);
  image.src = objectUrl;
  try {
    await image.decode();
  } catch {
    throw new TypeError(t("image.decodeFailed"));
  }
  if (IMAGE_URLS.get(image) !== objectUrl) return;
  INLINE_IMAGES.add(image);
  refreshPinEditors();
  setImageStatus(image, `${image.naturalWidth} × ${image.naturalHeight} px`, null);
}

function installAttachmentDownload(image, objectUrl) {
  const download = imageDownloadElement(image);
  if (!download || !download.hasAttribute("download")) {
    throw new TypeError(t("image.noDownloadAction"));
  }

  // Attachment bytes are never assigned to img/frame/navigation. The Blob URL
  // exists only on an <a download> action and is revoked just after activation.
  image.removeAttribute("src");
  download.setAttribute("href", objectUrl);
  download.hidden = false;
  download.onclick = () => {
    window.setTimeout(() => {
      if (IMAGE_URLS.get(image) !== objectUrl) return;
      revokeImageUrl(image);
      setImageStatus(image, t("image.downloaded"), null);
    }, 0);
  };
  setImageStatus(image, t("image.attachmentOnly"), "warning");
}

async function responseProblem(response) {
  const type = response.headers.get("Content-Type")?.split(";", 1)[0].trim().toLowerCase();
  if (type === "application/problem+json" || type === "application/json") {
    try {
      return new SynapseApiError(await response.json(), response);
    } catch (error) {
      if (error instanceof SynapseApiError) return error;
    }
  }
  return new SynapseApiError(null, response);
}

async function loadApiImage(image) {
  const source = image.dataset.url;
  if (!source) {
    setImageStatus(image, t("image.sourceUnavailable"), "error");
    return;
  }

  const controller = new AbortController();
  IMAGE_REQUESTS.set(image, controller);
  revokeImageUrl(image);
  image.removeAttribute("src");
  image.setAttribute("aria-busy", "true");
  setImageStatus(image, image.dataset.loadingMessage || t("image.loading"), null);

  try {
    const response = await apiFetch(source, {
      headers: { Accept: [...ALLOWED_RASTER_TYPES, ATTACHMENT_MEDIA_TYPE].join(", "), ...(image.dataset.sourceConfirmation ? { "X-Synapse-Source-Confirmation": image.dataset.sourceConfirmation } : {}) },
      signal: controller.signal,
    });
    if (!response.ok) throw await responseProblem(response);

    const type = response.headers.get("Content-Type")?.trim().toLowerCase();
    const disposition = response.headers.get("Content-Disposition")?.trim().toLowerCase() || "";
    const responseMode = type ? creatorImageResponseMode(type, disposition) : null;
    if (!responseMode) {
      throw new TypeError(t("image.unsafeResponse"));
    }

    const contentLength = response.headers.get("Content-Length");
    if (contentLength) {
      if (!/^\d+$/u.test(contentLength) || BigInt(contentLength) > BigInt(MAX_IMAGE_BYTES)) {
        throw new TypeError(t("image.tooLarge"));
      }
    }

    const blob = await response.blob();
    if (blob.size > MAX_IMAGE_BYTES) {
      throw new TypeError(t("image.tooLarge"));
    }
    if (controller.signal.aborted || IMAGE_REQUESTS.get(image) !== controller) return;

    revokeImageUrl(image);
    const objectUrl = URL.createObjectURL(blob);
    IMAGE_URLS.set(image, objectUrl);
    if (responseMode === "inline") await installInlineRaster(image, objectUrl);
    else installAttachmentDownload(image, objectUrl);
  } catch (error) {
    if (
      IMAGE_REQUESTS.get(image) === controller &&
      !(error instanceof DOMException && error.name === "AbortError")
    ) {
      revokeImageUrl(image);
      image.removeAttribute("src");
      setImageStatus(image, publicErrorMessage(error), "error");
    }
  } finally {
    if (IMAGE_REQUESTS.get(image) === controller) {
      IMAGE_REQUESTS.delete(image);
      image.setAttribute("aria-busy", "false");
      imageComparison?.refresh();
    }
  }
}

/** Load trusted session images through the authenticated API into revocable object URLs. */
export function enhanceApiImages(root = document) {
  for (const image of root.querySelectorAll("img[data-synapse-image]")) {
    if (!(image instanceof HTMLImageElement) || ENHANCED_IMAGES.has(image)) continue;
    ENHANCED_IMAGES.add(image);
    IMAGE_ELEMENTS.add(image);
    void loadApiImage(image);
  }
}

function releaseImageResources() {
  imageComparison?.release();
  for (const controller of IMAGE_REQUESTS.values()) controller.abort();
  IMAGE_REQUESTS.clear();

  for (const image of IMAGE_ELEMENTS) {
    revokeImageUrl(image);
    ENHANCED_IMAGES.delete(image);
  }
  IMAGE_ELEMENTS.clear();
  imageComparison?.refresh();
}

function releaseImagesWithin(root) {
  for (const image of root.querySelectorAll("img[data-synapse-image]")) {
    if (!(image instanceof HTMLImageElement)) continue;
    IMAGE_REQUESTS.get(image)?.abort();
    IMAGE_REQUESTS.delete(image);
    revokeImageUrl(image);
    ENHANCED_IMAGES.delete(image);
    IMAGE_ELEMENTS.delete(image);
  }
}

/** Compare decoded inline rasters; attachment Blob URLs never enter this view. */
export function enhanceImageComparison(root = document) {
  const section = root.querySelector("[data-synapse-comparison]");
  if (!section || imageComparison || typeof HTMLDialogElement === "undefined") return;
  const dialog = section.querySelector("dialog");
  if (!(dialog instanceof HTMLDialogElement) || typeof dialog.showModal !== "function") return;
  const opener = section.querySelector("[data-synapse-compare-open]");
  const status = section.querySelector("[data-synapse-compare-status]");
  const zoom = dialog.querySelector("[data-synapse-compare-zoom]");
  const modes = [...dialog.querySelectorAll("[data-synapse-compare-mode]")];
  const opacity = dialog.querySelector("[data-synapse-compare-opacity]");
  const opacityControl = dialog.querySelector("[data-synapse-compare-opacity-control]");
  const opacityValue = dialog.querySelector("[data-synapse-compare-opacity-value]");
  const overlayUnavailable = dialog.querySelector("[data-synapse-compare-overlay-unavailable]");
  const overlay = dialog.querySelector("[data-synapse-compare-overlay]");
  const overlayViewport = dialog.querySelector("[data-synapse-compare-overlay-viewport]");
  const overlayCanvas = dialog.querySelector("[data-synapse-compare-overlay-canvas]");
  const overlayImages = {
    a: dialog.querySelector("[data-synapse-compare-overlay-image-a]"),
    b: dialog.querySelector("[data-synapse-compare-overlay-image-b]"),
  };
  const overlayCaption = dialog.querySelector("[data-synapse-compare-overlay-caption]");
  const sources = [...section.querySelectorAll("img[data-synapse-image]")];
  // The selectors stay above both presentation modes, so changing modes never
  // hides the A/B choice or destroys its keyboard focus.
  const sourceSelectors = [...dialog.querySelectorAll("[data-synapse-compare-source]")];
  const panes = [...dialog.querySelectorAll("[data-synapse-compare-pane]")].map((pane, index) => ({
    select: sourceSelectors[index],
    image: pane.querySelector("[data-synapse-compare-image]"),
    caption: pane.querySelector("[data-synapse-compare-caption]"),
    viewport: pane.querySelector("[data-synapse-compare-viewport]"),
  }));
  const ready = (source) => source && INLINE_IMAGES.has(source) && IMAGE_URLS.has(source);
  const overlaySupported = () => {
    const a = sources[Number(panes[0].select.value)];
    const b = sources[Number(panes[1].select.value)];
    return ready(a) && ready(b) && a.naturalWidth === b.naturalWidth && a.naturalHeight === b.naturalHeight;
  };
  const selectedMode = () => modes.find((mode) => mode.checked)?.value || "side-by-side";
  const clear = () => {
    for (const pane of panes) {
      pane.image.removeAttribute("src");
      pane.image.hidden = true;
      pane.image.alt = "";
      pane.caption.textContent = "";
    }
    for (const image of Object.values(overlayImages)) image.removeAttribute("href");
    overlayCaption.textContent = "";
  };
  const renderOverlay = () => {
    const supported = overlaySupported();
    const isOverlay = selectedMode() === "overlay";
    overlay.hidden = !isOverlay || !supported;
    opacityControl.hidden = !isOverlay || !supported;
    dialog.dataset.comparisonMode = isOverlay && supported ? "overlay" : "side-by-side";
    if (!isOverlay) return;
    if (!supported) {
      overlayCaption.textContent = t("compare.overlayUnavailable");
      return;
    }
    const a = sources[Number(panes[0].select.value)];
    const b = sources[Number(panes[1].select.value)];
    const scale = zoom.value === "2" ? 2 : 1;
    const width = a.naturalWidth * scale;
    const height = a.naturalHeight * scale;
    overlayCanvas.setAttribute("width", String(width));
    overlayCanvas.setAttribute("height", String(height));
    overlayCanvas.setAttribute("viewBox", `0 0 ${width} ${height}`);
    for (const image of Object.values(overlayImages)) {
      image.setAttribute("width", String(width));
      image.setAttribute("height", String(height));
    }
    overlayImages.a.setAttribute("href", IMAGE_URLS.get(a));
    overlayImages.b.setAttribute("href", IMAGE_URLS.get(b));
    // SVG presentation attributes work under the no-inline-style CSP. They
    // multiply the image's own alpha rather than replacing transparent pixels.
    overlayImages.b.setAttribute("opacity", String(Number(opacity.value) / 100));
    overlayViewport.dataset.zoom = zoom.value === "fit" ? "fit" : "actual";
    overlayCaption.textContent = t("compare.overlayCaption", { a: a.dataset.label, b: b.dataset.label, opacity: opacity.value });
  };
  const render = () => {
    if (!dialog.open) return;
    for (const pane of panes) {
      const source = sources[Number(pane.select.value)];
      if (!ready(source)) {
        pane.image.removeAttribute("src");
        pane.image.hidden = true;
        pane.caption.textContent = t("compare.paneUnavailable");
        continue;
      }
      const url = IMAGE_URLS.get(source);
      if (pane.image.getAttribute("src") !== url) pane.image.src = url;
      pane.image.alt = source.alt;
      pane.image.hidden = false;
      pane.caption.textContent = `${source.dataset.label} · ${source.naturalWidth} × ${source.naturalHeight} px`;
      pane.viewport.dataset.zoom = zoom.value === "fit" ? "fit" : "actual";
      // Width/height attributes work under the existing no-inline-style CSP.
      const scale = zoom.value === "2" ? 2 : 1;
      pane.image.width = source.naturalWidth * scale;
      pane.image.height = source.naturalHeight * scale;
    }
    renderOverlay();
  };
  const refresh = () => {
    const count = sources.filter(ready).length;
    opener.disabled = count < 2;
    status.textContent = t(count >= 2 ? "compare.ready" : "compare.notReady");
    for (const pane of panes) {
      for (const option of pane.select.options) option.disabled = !ready(sources[Number(option.value)]);
    }
    const supported = overlaySupported();
    const overlayMode = modes.find((mode) => mode.value === "overlay");
    overlayMode.disabled = !supported;
    overlayUnavailable.hidden = supported;
    if (!supported && overlayMode.checked) modes.find((mode) => mode.value === "side-by-side").checked = true;
    render();
  };
  opener.addEventListener("click", () => {
    if (opener.disabled || dialog.open) return;
    const available = sources.map((source, index) => ready(source) ? index : -1).filter((index) => index >= 0);
    if (available.length < 2) return;
    // Prefer Current vs AI output. Unavailable roles stay visibly disabled.
    const current = sources.findIndex((source) => source.dataset.label === "Current" && ready(source));
    const output = sources.findIndex((source) => source.dataset.label === "AI output" && ready(source));
    panes[0].select.value = String(current >= 0 ? current : available[0]);
    panes[1].select.value = String(output >= 0 && output !== Number(panes[0].select.value)
      ? output : available.find((index) => index !== Number(panes[0].select.value)));
    zoom.value = "fit";
    modes.find((mode) => mode.value === "side-by-side").checked = true;
    opacity.value = "50";
    opacity.setAttribute("aria-valuenow", "50");
    opacityValue.value = "50%";
    opacityValue.textContent = "50%";
    dialog.showModal();
    refresh();
    for (const pane of panes) pane.viewport.scrollTo(0, 0);
  });
  dialog.querySelector("[data-synapse-compare-close]").addEventListener("click", () => dialog.close());
  dialog.addEventListener("close", () => {
    clear();
    opener.focus({ preventScroll: true });
  });
  for (const pane of panes) {
    pane.select.addEventListener("change", () => {
      refresh();
      pane.viewport.scrollTo(0, 0);
      overlayViewport.scrollTo(0, 0);
    });
  }
  zoom.addEventListener("change", () => {
    render();
    for (const pane of panes) pane.viewport.scrollTo(0, 0);
    overlayViewport.scrollTo(0, 0);
  });
  for (const mode of modes) mode.addEventListener("change", () => {
    render();
    overlayViewport.scrollTo(0, 0);
  });
  opacity.addEventListener("input", () => {
    opacity.setAttribute("aria-valuenow", opacity.value);
    opacityValue.value = `${opacity.value}%`;
    opacityValue.textContent = `${opacity.value}%`;
    renderOverlay();
  });
  imageComparison = {
    refresh,
    release() {
      if (dialog.open) dialog.close();
      clear();
    },
  };
  opener.hidden = false;
  status.hidden = false;
  refresh();
}

function enhanceCreatorSessionFilter(root = document) {
  const list = root.querySelector("[data-creator-session-list]");
  if (!list) return;
  enhanceCreatorSessionLocator(list);
  const filter = list?.querySelector("[data-creator-session-filter]");
  const count = list?.querySelector("[data-creator-session-count]");
  if (!filter || !count) return;
  const rows = [...list.querySelectorAll("[data-creator-session-row]")];
  const applyFilter = () => {
    const selected = filter.value;
    let visible = 0;
    for (const row of rows) {
      const matches = selected === "all"
        || row.dataset.state === selected
        || (row.dataset.disposition || "").toLowerCase() === selected;
      row.hidden = !matches;
      visible += Number(matches);
    }
    count.textContent = t(visible === 1 ? "sessions.countOne" : "sessions.count", { count: visible });
  };
  filter.addEventListener("change", applyFilter);
  window.addEventListener("pageshow", applyFilter);
  applyFilter();
}

function enhanceCreatorSessionLocator(list) {
  const locator = list.querySelector("[data-creator-session-locator]");
  const form = locator?.querySelector("[data-creator-session-locator-form]");
  const input = form?.elements.namedItem("session_name");
  const basePath = locator?.dataset.sessionBase;
  if (!(form instanceof HTMLFormElement) || !(input instanceof HTMLInputElement) || !basePath) return;

  let base;
  try {
    base = new URL(basePath, window.location.origin);
  } catch {
    return;
  }
  if (base.origin !== window.location.origin || base.pathname !== basePath || base.search || base.hash) return;

  const updateValidity = () => {
    input.setCustomValidity(CREATOR_SESSION_SLUG.test(input.value) ? "" : input.dataset.invalidMessage || "");
  };
  input.addEventListener("input", updateValidity);
  form.addEventListener("submit", (event) => {
    updateValidity();
    if (!input.checkValidity()) {
      event.preventDefault();
      input.reportValidity();
      return;
    }
    event.preventDefault();
    const target = new URL(`${base.pathname}${encodeURIComponent(input.value)}`, window.location.origin);
    if (target.origin === window.location.origin && target.pathname.startsWith(base.pathname)) window.location.assign(target.pathname);
  });
  locator.hidden = false;
}

// The local chooser hint matches the server's raster signature allowlist.
// It grants no authority: the server still verifies/classifies the submitted bytes.
function localPreviewMediaType(bytes) {
  const starts = (signature) => signature.every((value, index) => bytes[index] === value);
  if (starts([137, 80, 78, 71, 13, 10, 26, 10])) return "image/png";
  if (starts([255, 216, 255])) return "image/jpeg";
  const text = String.fromCharCode(...bytes);
  if (text.startsWith("GIF87a") || text.startsWith("GIF89a")) return "image/gif";
  if (bytes.length >= 12 && text.startsWith("RIFF") && text.slice(8, 12) === "WEBP") return "image/webp";
  return null;
}

function fileSizeLabel(bytes) {
  return `${bytes.toLocaleString("en-US")} bytes (${(bytes / 1024 / 1024).toFixed(2)} MiB)`;
}

function updateUtf8Counter(input, counter, limit) {
  const count = UTF8_ENCODER.encode(input.value).byteLength;
  const error = count > limit ? t("text.tooLong", { limit, count }) : "";
  input.setCustomValidity(error);
  input.setAttribute("aria-invalid", String(Boolean(error)));
  counter.hidden = false;
  counter.textContent = `${count} / ${limit} bytes${error ? ` · ${error}` : ""}`;
  if (error) counter.dataset.tone = "error";
  else delete counter.dataset.tone;
}

/** Optional preflight feedback only; multipart/server validation remains authoritative. */
export function enhanceCreatorUploads(root = document) {
  for (const form of root.querySelectorAll("form[data-synapse-creator-upload]")) {
    if (!(form instanceof HTMLFormElement) || CREATOR_UPLOADS.has(form)) continue;
    let active = true;
    const states = new Map();
    const fields = [...form.querySelectorAll("[data-creator-file]")].map((field) => ({
      input: field.querySelector('input[type="file"]'),
      image: field.querySelector("[data-creator-preview]"),
      info: field.querySelector("[data-creator-file-info]"),
      status: field.querySelector("[data-creator-preview-status]"),
      clear: field.querySelector("[data-creator-file-clear]"),
    }));
    const release = (field) => {
      const state = states.get(field.input);
      states.delete(field.input);
      field.image.removeAttribute("src");
      field.image.hidden = true;
      if (state?.url) URL.revokeObjectURL(state.url);
    };
    const summary = () => {
      const files = fields.flatMap(({ input }) => [...input.files]);
      const target = form.querySelector("[data-creator-file-summary]");
      target.textContent = t("upload.summary", {
        selected: files.length,
        total: fields.length,
        size: fileSizeLabel(files.reduce((total, file) => total + file.size, 0)),
        limit: fields.length * 64,
      });
      target.hidden = false;
    };
    const update = async (field) => {
      if (!active) return;
      release(field);
      const file = field.input.files[0];
      const state = {};
      states.set(field.input, state);
      const isCurrent = () => active && states.get(field.input) === state;
      field.info.hidden = !file;
      field.info.textContent = file ? `${file.name} · ${fileSizeLabel(file.size)}` : "";
      field.clear.hidden = !file;
      field.status.hidden = !file;
      field.status.textContent = "";
      delete field.status.dataset.tone;
      const error = file?.size > MAX_IMAGE_BYTES ? t("upload.fileTooLarge") : "";
      field.input.setCustomValidity(error);
      field.input.setAttribute("aria-invalid", String(Boolean(error)));
      summary();
      if (!file) return;
      if (error) {
        field.status.textContent = error;
        field.status.dataset.tone = "error";
        return;
      }
      field.status.textContent = t("upload.previewLoading");
      try {
        // Read only the signature before allocating a URL. File.type and the
        // filename are caller supplied and never authorize inline rendering.
        const type = localPreviewMediaType(new Uint8Array(await file.slice(0, 12).arrayBuffer()));
        if (!isCurrent()) return;
        if (!type) {
          field.status.textContent = t("upload.previewUnsupported");
          return;
        }
        state.url = URL.createObjectURL(file.slice(0, file.size, type));
        field.image.src = state.url;
        await field.image.decode();
        if (!isCurrent()) return;
        field.image.hidden = false;
        field.status.textContent = t("upload.previewReady", { width: field.image.naturalWidth, height: field.image.naturalHeight });
      } catch {
        if (!isCurrent()) return;
        release(field);
        field.status.textContent = t("upload.previewFailed");
      }
    };
    const updateText = () => {
      for (const counter of form.querySelectorAll("[data-creator-text-count]")) {
        const name = counter.dataset.creatorTextCount;
        const input = form.elements.namedItem(name);
        const limit = CREATOR_TEXT_FIELDS.get(name);
        updateUtf8Counter(input, counter, limit);
      }
    };
    const refresh = () => {
      active = true;
      updateText();
      for (const field of fields) void update(field);
    };
    for (const field of fields) {
      field.input.addEventListener("change", () => void update(field));
      field.clear.addEventListener("click", () => {
        field.input.value = "";
        void update(field);
        field.input.focus();
      });
    }
    form.addEventListener("input", updateText);
    // The reset event fires before the browser restores the controls' values.
    form.addEventListener("reset", () => window.setTimeout(() => { if (active) refresh(); }, 0));
    form.querySelector("[data-creator-preview-note]").hidden = false;
    CREATOR_UPLOADS.set(form, {
      refresh,
      release() {
        active = false;
        for (const field of fields) release(field);
      },
    });
    refresh();
  }
}

function releaseCreatorPreviews() {
  for (const upload of CREATOR_UPLOADS.values()) upload.release();
}

function formJson(form, submitter) {
  const payload = Object.create(null);
  const data = formDataWithSubmitter(form, submitter);

  for (const [name, value] of data) {
    if (value instanceof File) {
      throw new TypeError(t("form.fileNeedsUpload"));
    }
    if (Object.hasOwn(payload, name)) {
      throw new TypeError(t("form.fieldOnce", { name }));
    }
    const control = form.elements.namedItem(name);
    if (control instanceof HTMLElement && control.dataset.maxUtf8Bytes) {
      const limit = Number(control.dataset.maxUtf8Bytes);
      if (!Number.isSafeInteger(limit) || limit < 0 || UTF8_ENCODER.encode(value).byteLength > limit) {
        throw new TypeError(t("form.fieldTooLong", { name }));
      }
    }
    payload[name] = value;
  }

  if (form.dataset.synapseDecision === "true") {
    const editor = PIN_EDITORS.get(form);
    if (editor) {
      const annotations = editor.payload();
      if (annotations) payload.annotations = annotations;
    }
    if (UTF8_ENCODER.encode(JSON.stringify(payload)).byteLength > 8192) throw new TypeError(t("form.decisionTooLarge"));
  }
  return payload;
}

function archiveRestoreJson(form, submitter) {
  const expectedProjectKey = form.dataset.restoreProjectKey;
  if (typeof expectedProjectKey !== "string" || !/^[a-z][a-z0-9-]{0,63}$/u.test(expectedProjectKey)) {
    throw new TypeError(t("restore.targetInvalid"));
  }

  const fields = new Map();
  for (const [name, value] of formDataWithSubmitter(form, submitter)) {
    if (
      value instanceof File ||
      !["archive_name", "confirm_target_project_key", "confirm_empty_target"].includes(name) ||
      fields.has(name)
    ) {
      throw new TypeError(t("restore.confirmationInvalid"));
    }
    fields.set(name, value);
  }

  const archiveName = fields.get("archive_name");
  const targetProjectKey = fields.get("confirm_target_project_key");
  if (
    fields.size !== 3 ||
    typeof archiveName !== "string" ||
    !/^[a-z][a-z0-9-]{0,63}$/u.test(archiveName) ||
    targetProjectKey !== expectedProjectKey ||
    fields.get("confirm_empty_target") !== "true"
  ) {
    throw new TypeError(t("restore.confirmationInvalid"));
  }

  return {
    archive_name: archiveName,
    confirm_target_project_key: targetProjectKey,
    confirm_empty_target: true,
  };
}

function formDataWithSubmitter(form, submitter) {
  const data = new FormData(form);
  if (submitter) {
    if (
      !(submitter instanceof HTMLButtonElement || submitter instanceof HTMLInputElement) ||
      submitter.form !== form
    ) {
      throw new TypeError(t("form.submitterInvalid"));
    }
    if (submitter.name) data.append(submitter.name, submitter.value);
  }
  return data;
}

function creatorMultipart(form, submitter) {
  const source = formDataWithSubmitter(form, submitter);
  const derived = form.dataset.creatorDerived === "true";
  const textFields = new Map(CREATOR_TEXT_FIELDS);
  if (derived) textFields.set("confirmation_id", 64);
  const fileFields = derived ? new Set(["ai_output"]) : CREATOR_FILE_FIELDS;
  const text = new Map();
  const files = new Map();
  let aggregateBytes = 0;

  for (const [name, value] of source) {
    if (textFields.has(name)) {
      if (typeof value !== "string" || text.has(name)) {
        throw new TypeError(t("form.textFieldOnce", { name }));
      }
      const byteLength = UTF8_ENCODER.encode(value).byteLength;
      if ((!name.startsWith("generation_") && byteLength === 0) || byteLength > textFields.get(name)) {
        throw new TypeError(t("form.fieldTooLong", { name }));
      }
      text.set(name, value);
      continue;
    }

    if (fileFields.has(name)) {
      if (!(value instanceof File) || files.has(name)) {
        throw new TypeError(t("form.fileFieldOnce", { name }));
      }
      if (value.size > MAX_IMAGE_BYTES) {
        throw new TypeError(t("form.fileTooLarge", { name }));
      }
      aggregateBytes += value.size;
      if (aggregateBytes > MAX_UPLOAD_AGGREGATE_BYTES) {
        throw new TypeError(t("form.aggregateTooLarge"));
      }
      files.set(name, value);
      continue;
    }

    throw new TypeError(t("form.fieldNotAllowed", { name }));
  }

  const note = Object.fromEntries(["tool", "model", "prompt", "intent"].map(key => [key, text.get(`generation_${key}`) || ""]));
  if (UTF8_ENCODER.encode(JSON.stringify(note)).byteLength > 16384) throw new TypeError(t("note.tooLarge"));
  if (!["session", "subject_label", "creator_name"].every(name => text.has(name)) || files.size !== fileFields.size) {
    throw new TypeError(t("form.uploadFieldsInvalid"));
  }
  if (!/^[a-z][a-z0-9-]{0,63}$/u.test(text.get("session"))) {
    throw new TypeError(t("form.sessionInvalid"));
  }

  if (derived && !/^[0-9a-f]{64}$/u.test(text.get("confirmation_id") || "")) throw new TypeError(t("form.sourceConfirmationInvalid"));
  const normalized = new FormData();
  for (const name of textFields.keys()) {
    normalized.append(
      name,
      new Blob([text.get(name) || ""], { type: "text/plain; charset=utf-8" }),
      `${name}.txt`,
    );
  }
  for (const name of fileFields) {
    normalized.append(
      name,
      new Blob([files.get(name)], { type: "application/octet-stream" }),
      `${name}.bin`,
    );
  }
  return normalized;
}

export function formRequest(form, submitter) {
  const method = (submitter?.formMethod || form.method || "get").toUpperCase();
  // Chrome resolves a button without a `formaction` attribute to the current
  // document URL, not to its owning form's action. Only an explicit submitter
  // override may replace the API form action.
  const action = submitter?.hasAttribute("formaction") ? submitter.formAction : form.action;
  const url = resolveApiUrl(action);

  if (method === "GET") {
    const fields = formDataWithSubmitter(form, submitter);
    const fieldNames = new Set();
    for (const [name, value] of fields) {
      if (typeof value !== "string") throw new TypeError(t("form.getWithFile"));
      if (fieldNames.has(name)) {
        throw new TypeError(t("form.fieldOnce", { name }));
      }
      fieldNames.add(name);
      url.searchParams.append(name, value);
    }
    return { url, init: { method } };
  }

  if (form.dataset.synapseApiForm === "multipart") {
    return {
      url,
      init: {
        method,
        // The browser supplies the random boundary. Every part is rebuilt
        // above with the exact content type frozen by the local API contract.
        body: creatorMultipart(form, submitter),
      },
    };
  }

  if (form.dataset.synapseApiForm !== "json") {
    throw new TypeError(t("form.unknownEnhancement"));
  }

  const payload = form.dataset.archiveRestore === "true" ? archiveRestoreJson(form, submitter) : formJson(form, submitter);

  return {
    url,
    init: {
      method,
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(payload),
    },
  };
}

function statusElement(form) {
  const status = form.querySelector("[data-synapse-status]");
  return status instanceof HTMLElement ? status : null;
}

function setStatus(form, message, tone) {
  const status = statusElement(form);
  if (!status) return;
  status.textContent = message;
  status.hidden = message.length === 0;
  if (tone) status.dataset.tone = tone;
  else delete status.dataset.tone;
}

function setBusy(form, busy) {
  form.setAttribute("aria-busy", String(busy));
  const lockInputs = form.dataset.synapseCreatorUpload !== undefined || form.dataset.synapseDecision === "true";
  const controls = lockInputs ? "input, button, select, textarea" : "[data-synapse-submit]";
  for (const control of form.querySelectorAll(controls)) {
    if ("disabled" in control) {
      if (busy) {
        CONTROL_DISABLED_STATE.set(control, control.disabled);
        control.disabled = true;
      } else {
        control.disabled = CONTROL_DISABLED_STATE.get(control) ?? control.disabled;
        CONTROL_DISABLED_STATE.delete(control);
      }
    }
  }
  if (form.dataset.synapseDecision === "true") PIN_EDITORS.get(form)?.setBusy(busy);
}

function restoreSuccessLink(form) {
  const link = form.querySelector("[data-synapse-restore-success-link]");
  return link instanceof HTMLAnchorElement ? link : null;
}

function hideRestoreSuccessLink(form) {
  const link = restoreSuccessLink(form);
  if (link) link.hidden = true;
}

function lockCompletedArchiveRestore(form) {
  for (const control of form.querySelectorAll("input, button, select, textarea")) {
    if ("disabled" in control) control.disabled = true;
  }
}

// Stable problem codes with a localized summary. Other codes use a generic
// summary. The server's own problem text is appended unchanged as detail.
const LOCALIZED_ERROR_CODES = new Set(
  MESSAGE_ENTRIES.map(([key]) => key).filter((key) => key.startsWith("error.code.")).map((key) => key.slice(11)),
);

function problemMessage(code, detail, status) {
  const summary = LOCALIZED_ERROR_CODES.has(code)
    ? t(`error.code.${code}`)
    : typeof status === "number" && typeof detail !== "string"
      ? t("error.http", { status })
      : t("error.generic");
  return typeof detail === "string" && detail ? t("error.withDetail", { summary, detail }) : summary;
}

function operationFailureMessage(problem) {
  const detail = typeof problem?.detail === "string" ? problem.detail : null;
  if (!detail && !LOCALIZED_ERROR_CODES.has(problem?.code)) return t("operation.failed");
  return problemMessage(problem?.code, detail);
}

function publicErrorMessage(error) {
  if (error instanceof SynapseApiError) {
    return problemMessage(error.problem ? error.code : null, error.serverDetail, error.status);
  }
  if (error instanceof TypeError) return error.message;
  if (error instanceof DOMException && error.name === "AbortError") return t("error.cancelled");
  return t("error.unexpected");
}

function showCommittedReceipt(form, data) {
  const receipt = data?.receipt;
  if (!receipt || typeof receipt !== "object" || typeof receipt.decision_head !== "string") {
    throw new TypeError(t("decision.receiptInvalid"));
  }

  setStatus(form, t("decision.committedWithoutReport", { head: receipt.decision_head }), "success");
  let output = form.querySelector("[data-synapse-committed-receipt]");
  if (!(output instanceof HTMLElement)) {
    output = document.createElement("pre");
    output.dataset.synapseCommittedReceipt = "";
    output.className = "committed-receipt";
    form.after(output);
  }
  output.textContent = JSON.stringify(receipt, null, 2);
  output.hidden = false;
}

export async function submitEnhancedForm(event) {
  const form = event.currentTarget;
  if (!(form instanceof HTMLFormElement)) return;

  event.preventDefault();
  if (form.dataset.archiveRestore === "true" && COMPLETED_ARCHIVE_RESTORES.has(form)) return;
  if (form.getAttribute("aria-busy") === "true") return;
  hideRestoreSuccessLink(form);
  if (!form.reportValidity()) return;

  let prepared;
  try {
    prepared = formRequest(form, event.submitter);
  } catch (error) {
    setStatus(form, publicErrorMessage(error), "error");
    form.dispatchEvent(
      new CustomEvent("synapse:api-error", {
        bubbles: true,
        detail: { error },
      }),
    );
    return;
  }

  if (event.submitter?.name === "disposition") {
    const disposition = event.submitter.value;
    const session = form.dataset.confirmSession || t("decision.thisSession");
    const project = form.dataset.confirmProject;
    const summary = event.submitter.dataset.decisionSummary;
    const target = project
      ? t("decision.targetWithProject", { project, session })
      : t("decision.target", { session });
    const confirmed = window.confirm([
      t("decision.confirmPublish", { target, disposition }),
      ...(summary ? [summary, t("decision.finality")] : []),
      t("decision.confirmContinue"),
    ].join("\n"));
    if (!confirmed) {
      setStatus(form, t("decision.notSent"), null);
      return;
    }
  }

  if (form.dataset.confirmMaintenance === "fsck") {
    const confirmed = window.confirm(t("fsck.confirm"));
    if (!confirmed) {
      setStatus(form, t("fsck.notStarted"), null);
      return;
    }
  }

  if (form.dataset.confirmMaintenance === "archive-export") {
    const archiveName = new FormData(form).get("archive_name");
    if (typeof archiveName !== "string" || !/^[a-z][a-z0-9-]{0,63}$/u.test(archiveName)) {
      setStatus(form, t("export.notStarted"), null);
      return;
    }
    const confirmed = window.confirm(t("export.confirm", { name: archiveName }));
    if (!confirmed) {
      setStatus(form, t("export.notStarted"), null);
      return;
    }
  }

  let archiveRestore;
  if (form.dataset.archiveRestore === "true") {
    try {
      const restore = archiveRestoreJson(form, event.submitter);
      archiveRestore = {
        archiveName: restore.archive_name,
        projectKey: restore.confirm_target_project_key,
        kind: "archive_restore",
      };
    } catch (error) {
      setStatus(form, publicErrorMessage(error), "error");
      return;
    }
    const confirmed = window.confirm(
      t("restore.confirm", { archive: archiveRestore.archiveName, project: archiveRestore.projectKey }),
    );
    if (!confirmed) {
      setStatus(form, t("restore.notStarted"), null);
      return;
    }
  }

  setBusy(form, true);
  setStatus(form, form.dataset.busyMessage || t("form.working"), null);

  try {
    let destination = form.dataset.successLocation
      ? new URL(form.dataset.successLocation, window.location.origin)
      : null;
    if (destination && destination.origin !== window.location.origin) {
      throw new TypeError(t("form.destinationInvalid"));
    }

    let data = await apiJson(prepared.url, prepared.init);
    if (archiveRestore) {
      if (!validOperationAccepted(data)) {
        throw new TypeError(t("operation.restoreReceiptInvalid"));
      }
      setStatus(form, t("operation.restoreStarted"), null);
      data = await pollOperation(form, data, archiveRestore);
    } else if (data?.state === "queued") {
      setStatus(form, t("operation.started"), null);
      data = await pollOperation(form, data);
    }
    const committedWithoutReport = data?.state === "committed";
    if (committedWithoutReport) {
      showCommittedReceipt(form, data);
      destination = null;
    } else {
      setStatus(
        form,
        operationSuccessMessage(data, archiveRestore) || form.dataset.successMessage || t("form.completed"),
        "success",
      );
      if (archiveRestore) {
        const link = restoreSuccessLink(form);
        if (link) link.hidden = false;
        COMPLETED_ARCHIVE_RESTORES.add(form);
      }
    }

    if (form.dataset.successSessionBase) {
      const sessionBase = new URL(form.dataset.successSessionBase, window.location.origin);
      if (
        sessionBase.origin !== window.location.origin ||
        sessionBase.search ||
        sessionBase.hash ||
        typeof data?.session !== "string" ||
        !/^[a-z][a-z0-9-]{0,63}$/u.test(data.session)
      ) {
        throw new TypeError(t("form.sessionDestinationInvalid"));
      }
      destination = new URL(encodeURIComponent(data.session), sessionBase);
    }

    form.dispatchEvent(
      new CustomEvent("synapse:api-success", {
        bubbles: true,
        detail: { data },
      }),
    );

    if (destination) {
      window.location.assign(destination);
    } else if (!committedWithoutReport && form.dataset.successReload === "true") {
      window.location.reload();
    }
  } catch (error) {
    hideRestoreSuccessLink(form);
    setStatus(form, publicErrorMessage(error), "error");
    form.dispatchEvent(
      new CustomEvent("synapse:api-error", {
        bubbles: true,
        detail: { error },
      }),
    );
  } finally {
    setBusy(form, false);
    if (COMPLETED_ARCHIVE_RESTORES.has(form)) lockCompletedArchiveRestore(form);
  }
}

/** Enhance explicitly opted-in API forms while preserving their normal fallback. */
export function enhanceApiForms(root = document) {
  for (const form of root.querySelectorAll("form[data-synapse-api-form]")) {
    if (!(form instanceof HTMLFormElement) || ENHANCED_FORMS.has(form)) continue;
    const status = statusElement(form);
    if (status) {
      status.setAttribute("role", "status");
      status.setAttribute("aria-live", "polite");
    }
    ENHANCED_FORMS.add(form);
    if (form.dataset.synapseDecision === "true") {
      const input = form.elements.namedItem("rationale");
      const counter = form.querySelector("[data-decision-text-count]");
      const update = () => updateUtf8Counter(input, counter, 5000);
      input.addEventListener("input", update);
      form.addEventListener("reset", () => window.setTimeout(update, 0));
      update();
    }
    form.addEventListener("submit", submitEnhancedForm);
    form.hidden = false;
  }
}

const PIN_EDITORS = new Map();
const PIN_VIEWERS = new Set();
const PIN_FORMAT = "synapsegit-creator-decision-pins-v1";
const PIN_MAX = 1_000_000;

function refreshPinEditors() {
  for (const editor of PIN_VIEWERS) editor.refresh();
}

function enhanceCreatorPins(root = document) {
  for (const panel of root.querySelectorAll("[data-creator-pins]")) {
    if (panel.dataset.unavailable === "true") continue;
    const editable = panel.dataset.editable === "true";
    const form = editable ? root.querySelector('form[data-synapse-decision="true"]') : null;
    const sources = [...root.querySelectorAll("img[data-synapse-image]")];
    const roles = ["original", "current", "ai_output"];
    const sourceByRole = new Map(roles.map((role, index) => [role, sources[index]]));
    const selector = panel.querySelector("[data-pin-image-role]");
    const zoom = panel.querySelector("[data-pin-zoom]");
    const canvas = panel.querySelector("[data-pin-canvas]");
    const preview = panel.querySelector("[data-pin-preview]");
    const markers = panel.querySelector("[data-pin-markers]");
    const list = panel.querySelector("[data-pin-list]");
    const addButton = panel.querySelector("[data-pin-add]");
    const status = panel.querySelector("[data-pin-status]");
    let busy = false;
    let pins = [];
    try {
      const stored = JSON.parse(panel.dataset.annotations);
      if (stored !== null) {
        if (stored.format !== PIN_FORMAT || !Array.isArray(stored.pins) || stored.pins.length > 10) throw new Error();
        pins = stored.pins;
      }
      if (pins.some(pin => !validBinding(pin) || typeof pin.note !== "string" || !pin.note || UTF8_ENCODER.encode(pin.note).byteLength > 200)) throw new Error();
    } catch {
      list.replaceChildren();
      status.textContent = t("pins.unreadable");
      panel.querySelector("[data-pin-controls]").hidden = false;
      return;
    }
    function validBinding(pin) {
      return roles.includes(pin.role) && pin.blob_oid === sourceByRole.get(pin.role)?.dataset.oid &&
        [pin.x, pin.y].every(value => Number.isSafeInteger(value) && value >= 0 && value <= PIN_MAX);
    }
    function ready() {
      const source = sourceByRole.get(selector.value);
      return source && INLINE_IMAGES.has(source) && IMAGE_URLS.has(source);
    }
    function annotationPayload(validate = true) {
      if (pins.length === 0) return null;
      if (validate && (pins.length > 10 || pins.some(pin => !validBinding(pin) || !pin.note || UTF8_ENCODER.encode(pin.note).byteLength > 200))) {
        throw new TypeError(t("pins.invalid"));
      }
      return { format: PIN_FORMAT, pins: pins.map(({ role, blob_oid, x, y, note }) => ({ role, blob_oid, x, y, note })) };
    }
    function updateJsonCount() {
      if (!form) return;
      const counter = form.querySelector("[data-decision-json-count]");
      const base = { review_id: form.elements.namedItem("review_id").value, rationale: form.elements.namedItem("rationale").value };
      const annotations = annotationPayload(false);
      if (annotations) base.annotations = annotations;
      const invalidPins = pins.some(pin => !validBinding(pin) || !pin.note || UTF8_ENCODER.encode(pin.note).byteLength > 200);
      const remaining = [];
      for (const button of form.querySelectorAll('button[name="disposition"]')) {
        const bytes = UTF8_ENCODER.encode(JSON.stringify({ ...base, disposition: button.value })).byteLength;
        remaining.push(t("pins.remaining", { label: button.textContent, bytes: 8192 - bytes }));
        button.disabled = busy || invalidPins || bytes > 8192;
      }
      counter.textContent = `${t("pins.jsonSummary", { remaining: remaining.join(" / ") })}${invalidPins ? t("pins.fixInvalid") : ""}`;
    }
    function positionMarkers() {
      for (const marker of markers.children) {
        const pin = pins[Number(marker.dataset.pinIndex)];
        marker.style.left = `${pin.x / PIN_MAX * 100}%`;
        marker.style.top = `${pin.y / PIN_MAX * 100}%`;
        marker.setAttribute("aria-label", t("pins.markerLabel", { number: Number(marker.dataset.pinIndex) + 1, note: pin.note || t("pins.noteMissing") }));
      }
      for (const input of list.querySelectorAll("[data-pin-axis]")) input.value = pins[Number(input.dataset.pinIndex)][input.dataset.pinAxis];
    }
    function move(index, x, y) {
      if (busy || !editable) return;
      pins[index].x = Math.max(0, Math.min(PIN_MAX, Math.round(x)));
      pins[index].y = Math.max(0, Math.min(PIN_MAX, Math.round(y)));
      positionMarkers();
      updateJsonCount();
    }
    function fromPointer(event) {
      const rect = preview.getBoundingClientRect();
      return [Math.round((event.clientX - rect.left) / rect.width * PIN_MAX), Math.round((event.clientY - rect.top) / rect.height * PIN_MAX)];
    }
    function remove(index) {
      if (busy) return;
      pins.splice(index, 1);
      renderList();
      refresh();
      addButton?.focus();
    }
    function renderMarkers() {
      markers.replaceChildren();
      if (!ready()) return;
      pins.forEach((pin, index) => {
        if (pin.role !== selector.value) return;
        const marker = document.createElement("button");
        marker.type = "button";
        marker.className = "image-pin";
        marker.dataset.pinIndex = String(index);
        marker.textContent = String(index + 1);
        marker.disabled = busy;
        if (editable) {
          marker.addEventListener("keydown", event => {
            const delta = event.shiftKey ? 1000 : 10000;
            const directions = { ArrowLeft: [-delta, 0], ArrowRight: [delta, 0], ArrowUp: [0, -delta], ArrowDown: [0, delta] };
            if (directions[event.key]) { event.preventDefault(); const [dx, dy] = directions[event.key]; move(index, pin.x + dx, pin.y + dy); }
            if (event.key === "Delete" || event.key === "Backspace") { event.preventDefault(); remove(index); }
          });
          marker.addEventListener("pointerdown", event => {
            if (busy || event.button !== 0) return;
            event.preventDefault(); marker.focus(); marker.setPointerCapture(event.pointerId);
          });
          marker.addEventListener("pointermove", event => { if (marker.hasPointerCapture(event.pointerId)) move(index, ...fromPointer(event)); });
          marker.addEventListener("pointerup", event => { if (marker.hasPointerCapture(event.pointerId)) marker.releasePointerCapture(event.pointerId); });
        }
        markers.append(marker);
      });
      positionMarkers();
    }
    function renderList() {
      markers.replaceChildren();
      list.replaceChildren();
      pins.forEach((pin, index) => {
        const item = document.createElement("li");
        const show = document.createElement("button");
        show.type = "button"; show.dataset.variant = "secondary";
        show.textContent = t("pins.show", { number: index + 1, role: pin.role });
        show.addEventListener("click", () => { selector.value = pin.role; refresh(); markers.querySelector(`[data-pin-index="${index}"]`)?.focus(); });
        item.append(show);
        if (editable) {
          const label = document.createElement("label"); label.textContent = t("pins.noteLabel", { number: index + 1 });
          const text = document.createElement("textarea"); text.id = `pin-note-${index}`; text.rows = 2; text.value = pin.note; label.htmlFor = text.id;
          const count = document.createElement("span"); count.className = "field__hint";
          const updateNote = () => { pin.note = text.value; const bytes = UTF8_ENCODER.encode(pin.note).byteLength; count.textContent = `${bytes} / 200 bytes`; text.setAttribute("aria-invalid", String(bytes === 0 || bytes > 200)); positionMarkers(); updateJsonCount(); };
          text.addEventListener("input", updateNote);
          item.append(label, text, count);
          for (const axis of ["x", "y"]) {
            const axisLabel = document.createElement("label"); axisLabel.textContent = t("pins.axisLabel", { number: index + 1, axis: axis.toUpperCase() });
            const input = document.createElement("input"); input.type = "number"; input.min = "0"; input.max = String(PIN_MAX); input.step = "1"; input.value = pin[axis]; input.id = `pin-${axis}-${index}`; axisLabel.htmlFor = input.id;
            input.dataset.pinAxis = axis; input.dataset.pinIndex = String(index);
            input.addEventListener("input", () => {
              pin[axis] = input.value === "" ? NaN : Number(input.value);
              input.setAttribute("aria-invalid", String(!validBinding(pin)));
              const marker = markers.querySelector(`[data-pin-index="${index}"]`);
              if (marker) marker.style[axis === "x" ? "left" : "top"] = `${pin[axis] / PIN_MAX * 100}%`;
              updateJsonCount();
            });
            item.append(axisLabel, input);
          }
          const deletion = document.createElement("button"); deletion.type = "button"; deletion.textContent = t("pins.delete", { number: index + 1 }); deletion.dataset.variant = "secondary";
          deletion.addEventListener("click", () => remove(index)); item.append(deletion);
          updateNote();
        } else {
          const text = document.createElement("p"); text.className = "decision-rationale"; text.textContent = `(${pin.x}, ${pin.y}) ${pin.note}`; item.append(text);
        }
        list.append(item);
      });
      updateJsonCount();
    }
    function refresh() {
      const source = sourceByRole.get(selector.value);
      const available = ready();
      preview.hidden = !available;
      if (available) {
        const url = IMAGE_URLS.get(source);
        if (preview.getAttribute("src") !== url) preview.src = url;
        canvas.style.width = zoom.value === "fit" ? `${source.naturalWidth}px` : `${source.naturalWidth * Number(zoom.value)}px`;
        canvas.style.maxWidth = zoom.value === "fit" ? "100%" : "none";
        status.textContent = t("pins.status", { label: source.dataset.label, width: source.naturalWidth, height: source.naturalHeight });
      } else {
        preview.removeAttribute("src");
        status.textContent = t("pins.unavailable");
      }
      if (addButton) addButton.disabled = busy || !available || pins.length >= 10;
      renderMarkers();
      updateJsonCount();
    }
    function add(x, y) {
      if (!editable || busy || !ready() || pins.length >= 10) return;
      pins.push({ role: selector.value, blob_oid: sourceByRole.get(selector.value).dataset.oid, x: Math.max(0, Math.min(PIN_MAX, x)), y: Math.max(0, Math.min(PIN_MAX, y)), note: "" });
      renderList(); refresh(); list.querySelector(`#pin-note-${pins.length - 1}`)?.focus();
    }
    addButton?.addEventListener("click", () => add(PIN_MAX / 2, PIN_MAX / 2));
    preview.addEventListener("click", event => add(...fromPointer(event)));
    selector.addEventListener("change", refresh);
    zoom.addEventListener("change", refresh);
    form?.addEventListener("input", updateJsonCount);
    const editor = { refresh, payload: annotationPayload, setBusy(value) {
      busy = value;
      for (const control of panel.querySelectorAll("button, input, select, textarea")) control.disabled = busy;
      refresh();
    } };
    PIN_VIEWERS.add(editor);
    if (form) PIN_EDITORS.set(form, editor);
    panel.querySelector("[data-pin-controls]").hidden = false;
    renderList(); refresh();
  }
}

function privateReportEndpoint(project, session) {
  const base = getApiBase().pathname;
  return `${base}/projects/${encodeURIComponent(project)}/creator-sessions/${encodeURIComponent(session)}`;
}

function isPrivateReportBinding(value) {
  return typeof value === "string" && value.length > 0 && value.length <= 512 && !/[\r\n]/u.test(value);
}

function verifiedPrivateReport(text, { session, proposalHead, decisionHead }) {
  let detail;
  try {
    detail = JSON.parse(text);
  } catch {
    return false;
  }
  const report = detail && typeof detail === "object" && !Array.isArray(detail) ? detail.report : null;
  return (
    detail?.state === "complete" &&
    report && typeof report === "object" && !Array.isArray(report) &&
    report.session === session &&
    report.proposal_head === proposalHead &&
    report.decision_head === decisionHead
  );
}

function releasePrivateReportResources() {
  for (const controller of PRIVATE_REPORT_REQUESTS) controller.abort();
  PRIVATE_REPORT_REQUESTS.clear();
  for (const objectUrl of PRIVATE_REPORT_URLS) URL.revokeObjectURL(objectUrl);
  PRIVATE_REPORT_URLS.clear();
}

/** Download one freshly verified complete-session Local API response. */
export function enhancePrivateReportDownload(root = document) {
  const section = root.querySelector("[data-private-report]");
  if (!(section instanceof HTMLElement) || section.dataset.privateReportEnhanced === "true") return;
  const button = section.querySelector("[data-private-report-download]");
  const status = section.querySelector("[data-private-report-status]");
  const { endpoint, projectKey: project, session, proposalHead, decisionHead } = section.dataset;
  if (!(button instanceof HTMLButtonElement) || !(status instanceof HTMLElement)) return;
  section.dataset.privateReportEnhanced = "true";

  const valid =
    typeof endpoint === "string" &&
    typeof project === "string" &&
    /^[a-z][a-z0-9-]{0,63}$/u.test(project) &&
    typeof session === "string" &&
    /^[a-z][a-z0-9-]{0,63}$/u.test(session) &&
    isPrivateReportBinding(proposalHead) &&
    isPrivateReportBinding(decisionHead) &&
    endpoint === privateReportEndpoint(project, session);
  if (!valid) {
    button.disabled = true;
    status.textContent = t("privateReport.invalid");
    return;
  }
  button.hidden = false;

  let busy = false;
  button.addEventListener("click", async () => {
    if (busy) return;
    busy = true;
    button.disabled = true;
    status.textContent = t("privateReport.preparing");
    const controller = new AbortController();
    PRIVATE_REPORT_REQUESTS.add(controller);
    try {
      const response = await apiFetch(endpoint, {
        method: "GET",
        headers: { Accept: "application/json" },
        signal: controller.signal,
      });
      if (!response.ok) throw await responseProblem(response);
      const contentType = response.headers.get("Content-Type")?.split(";", 1)[0].trim().toLowerCase();
      if (contentType !== "application/json") throw new TypeError(t("privateReport.invalid"));
      const text = await response.text();
      if (!verifiedPrivateReport(text, { session, proposalHead, decisionHead })) {
        throw new TypeError(t("privateReport.invalid"));
      }
      if (controller.signal.aborted) return;
      const objectUrl = URL.createObjectURL(new Blob([text], { type: "application/json;charset=utf-8" }));
      PRIVATE_REPORT_URLS.add(objectUrl);
      const link = document.createElement("a");
      link.href = objectUrl;
      link.download = `${session}-private-report.json`;
      document.body.append(link);
      link.click();
      link.remove();
      window.setTimeout(() => {
        URL.revokeObjectURL(objectUrl);
        PRIVATE_REPORT_URLS.delete(objectUrl);
      }, 1_000);
      status.textContent = t("privateReport.started");
    } catch (error) {
      if (!(error instanceof DOMException && error.name === "AbortError")) {
        status.textContent = error instanceof TypeError && error.message === t("privateReport.invalid")
          ? error.message
          : `${t("privateReport.invalid")} ${publicErrorMessage(error)}`;
      }
    } finally {
      PRIVATE_REPORT_REQUESTS.delete(controller);
      busy = false;
      button.disabled = false;
    }
  });
}

// Recorded times arrive as UTC text so pages stay readable without
// JavaScript. Show them in the browser's time zone; the title keeps the exact
// stored value.
function localizeRecordedTimes() {
  const format = new Intl.DateTimeFormat(document.documentElement.lang || undefined, {
    year: "numeric", month: "2-digit", day: "2-digit",
    hour: "2-digit", minute: "2-digit", second: "2-digit", timeZoneName: "short",
  });
  for (const element of document.querySelectorAll("time[data-local-time]")) {
    const date = new Date(element.dateTime);
    if (!Number.isNaN(date.getTime())) element.textContent = format.format(date);
  }
}

function start() {
  document.documentElement.classList.add("has-js");
  preserveLanguageSwitchQuery();
  localizeRecordedTimes();
  enhancePresentationForm();
  enhanceCreatorPins();
  enhanceApiForms();
  enhanceCreatorUploads();
  enhanceCreatorSessionFilter();
  enhanceImageComparison();
  enhanceApiImages();
  enhanceImportInbox();
  enhancePrivateReportDownload();
}

// The server removes only `lang` while preserving other page parameters. A
// relative `?lang=en` link would discard those parameters before that rule can
// run, so merge the current page query into each explicit language choice.
function preserveLanguageSwitchQuery() {
  const current = new URL(window.location.href);
  for (const link of document.querySelectorAll(".language-switch a[hreflang]")) {
    const locale = link.getAttribute("hreflang");
    if (locale !== "ja" && locale !== "en") continue;
    const destination = new URL(current);
    destination.searchParams.set("lang", locale);
    link.setAttribute("href", `${destination.pathname}${destination.search}${destination.hash}`);
  }
}

if (document.readyState === "loading") {
  document.addEventListener("DOMContentLoaded", start, { once: true });
} else {
  start();
}

window.addEventListener("pagehide", releaseImageResources);
window.addEventListener("pagehide", releaseCreatorPreviews);
window.addEventListener("pagehide", releasePrivateReportResources);
window.addEventListener("pageshow", (event) => {
  if (event.persisted) {
    enhanceApiImages();
    for (const upload of CREATOR_UPLOADS.values()) upload.refresh();
  }
});

function enhanceImportInbox() {
  const section = document.querySelector("[data-import-inbox]");
  if (!(section instanceof HTMLElement)) return;
  const project = section.dataset.projectKey;
  const status = section.querySelector("[data-import-inbox-status]");
  const list = section.querySelector("[data-import-inbox-list]");
  const form = section.querySelector("form[data-import-inbox-preview]");
  const images = section.querySelector("[data-import-inbox-images]");
  if (!project || !(status instanceof HTMLElement) || !(list instanceof HTMLElement) || !(form instanceof HTMLFormElement) || !(images instanceof HTMLElement)) return;
  let stageId = null;
  const endpoint = `/api/v1/projects/${encodeURIComponent(project)}/import-inbox`;
  const field = name => form.elements.namedItem(`inbox_${name}`);
  const note = () => { const value = { tool: field("generation_tool").value, model: field("generation_model").value, prompt: field("generation_prompt").value, intent: field("generation_intent").value }; return Object.values(value).every(value => value === "") ? null : value; };
  const showList = async () => {
    if (!stageId) return;
    const cancelled = stageId;
    status.textContent = t("inbox.discarding");
    try {
      const response = await apiFetch(`${endpoint}/stages/${encodeURIComponent(cancelled)}`, { method: "DELETE" });
      if (response.status !== 204) throw await responseProblem(response);
      releaseImagesWithin(images);
      images.replaceChildren();
      stageId = null;
      form.hidden = true;
      list.hidden = false;
      list.querySelectorAll("button").forEach(button => { button.disabled = false; });
      status.textContent = t("inbox.discarded");
    } catch (error) {
      status.textContent = t("inbox.discardFailed", { error: publicErrorMessage(error) });
    }
  };
  const renderImages = () => {
    images.replaceChildren();
    for (const [role, label] of [["original", "Original image"], ["current", "Current image"], ["ai-output", "AI output"]]) {
      const figure = document.createElement("figure"); const heading = document.createElement("figcaption"); heading.textContent = label;
      const image = document.createElement("img"); image.alt = t("inbox.stagedAlt", { label }); image.dataset.synapseImage = ""; image.dataset.url = `${endpoint}/stages/${encodeURIComponent(stageId)}/images/${role}`; image.dataset.label = label;
      const download = document.createElement("a"); download.hidden = true; download.dataset.synapseImageDownload = ""; download.textContent = t("inbox.download");
      figure.append(heading, image, download); images.append(figure);
    }
    enhanceApiImages(images);
  };
  const stage = async (slug, button) => {
    button.disabled = true; status.textContent = t("inbox.staging");
    try {
      const preview = await apiJson(`${endpoint}/${encodeURIComponent(slug)}/stages`, { method: "POST" });
      if (!preview || typeof preview.stage_id !== "string" || !/^[0-9a-f]{64}$/u.test(preview.stage_id)) throw new TypeError(t("inbox.stageInvalid"));
      stageId = preview.stage_id;
      for (const name of ["session", "subject_label", "creator_name"]) field(name).value = preview[name] || "";
      const generation = preview.generation_note || {}; field("generation_tool").value = generation.tool || ""; field("generation_model").value = generation.model || ""; field("generation_prompt").value = generation.prompt || ""; field("generation_intent").value = generation.intent || "";
      list.hidden = true; form.hidden = false; status.textContent = t("inbox.staged"); renderImages(); form.querySelector("h3")?.focus();
    } catch (error) { status.textContent = publicErrorMessage(error); button.disabled = false; }
  };
  const load = async () => {
    try {
      const result = await apiJson(endpoint); if (!Array.isArray(result?.items)) throw new TypeError(t("inbox.listInvalid"));
      section.hidden = false; list.replaceChildren();
      for (const item of result.items) {
        const row = document.createElement("li"); const title = document.createElement("strong"); title.textContent = item.slug;
        const detail = document.createElement("p"); detail.className = "field__hint"; detail.textContent = item.ready ? t("inbox.ready") : item.reason ? t("inbox.notReadyReason", { reason: item.reason }) : t("inbox.notReady");
        row.append(title, detail);
        if (item.ready) { const button = document.createElement("button"); button.type = "button"; button.textContent = t("inbox.review"); button.addEventListener("click", () => void stage(item.slug, button)); row.append(button); }
        list.append(row);
      }
      status.textContent = t(result.items.length ? "inbox.choose" : "inbox.empty");
    } catch (error) {
      if (error instanceof SynapseApiError && error.code === "service_unavailable") return;
      section.hidden = false; status.textContent = publicErrorMessage(error);
    }
  };
  form.addEventListener("submit", async event => {
    event.preventDefault(); if (!stageId || !form.reportValidity()) return;
    const bytes = UTF8_ENCODER.encode(JSON.stringify(note())).byteLength; if (bytes > 16384) { status.textContent = t("note.tooLarge"); return; }
    const submit = form.querySelector("[type=submit]"); if (submit) submit.disabled = true;
    try {
      const pending = await apiJson(`${endpoint}/stages/${encodeURIComponent(stageId)}/creator-sessions`, { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ session: field("session").value, subject_label: field("subject_label").value, creator_name: field("creator_name").value, generation_note: note() }) });
      if (!pending?.session) throw new TypeError(t("inbox.proposalInvalid"));
      window.location.assign(`/projects/${encodeURIComponent(project)}/creator-sessions/${encodeURIComponent(pending.session)}`);
    } catch (error) { status.textContent = publicErrorMessage(error); if (submit) submit.disabled = false; }
  });
  form.querySelector("[data-import-inbox-cancel]")?.addEventListener("click", () => void showList());
  void load();
}

function enhancePresentationForm() {
  const form = document.querySelector("[data-presentation-form]");
  if (!(form instanceof HTMLFormElement)) return;
  const preview = document.querySelector("[data-presentation-preview]");
  const status = form.querySelector("[data-presentation-status]");
  const download = preview.querySelector("[data-presentation-download]");
  let validatedToml = null;
  let revision = 0;
  let busy = false;
  const clearPreview = () => {
    revision += 1;
    validatedToml = null;
    preview.hidden = true;
    preview.querySelector("[data-presentation-text]").replaceChildren();
    preview.querySelector("[data-presentation-download-status]").textContent = "";
  };
  form.addEventListener("input", event => {
    clearPreview();
    event.target.setCustomValidity?.("");
    status.textContent = "";
  });
  form.addEventListener("change", clearPreview);
  form.addEventListener("submit", async event => {
    event.preventDefault();
    if (busy || !form.reportValidity()) return;
    clearPreview();
    const input = { session: form.elements.namedItem("session").value };
    const entries = [];
    for (const control of form.querySelectorAll("[data-public-max-bytes]")) {
      const bytes = UTF8_ENCODER.encode(control.value).length;
      const limit = Number(control.dataset.publicMaxBytes);
      if (bytes > limit) {
        const message = t("presentation.tooLong", { label: control.labels[0].textContent, bytes, limit });
        control.setCustomValidity(message); status.textContent = message; control.reportValidity(); return;
      }
      if (control.value !== "") {
        input[control.name] = control.value;
        entries.push([control.labels[0].textContent, control.value]);
      }
    }
    const currentRevision = revision;
    busy = true;
    const controls = [...form.querySelectorAll("input, select, textarea, button")];
    controls.forEach(control => { control.disabled = true; });
    status.textContent = t("presentation.validating");
    try {
      const result = await apiJson(form.dataset.endpoint, { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(input) });
      if (revision !== currentRevision) return;
      if (typeof result?.toml !== "string" || UTF8_ENCODER.encode(result.toml).length > 65536) throw new TypeError(t("presentation.responseInvalid"));
      validatedToml = result.toml;
      const list = preview.querySelector("[data-presentation-text]");
      for (const [label, value] of entries.length ? entries : [[t("presentation.emptyLabel"), t("presentation.emptyValue")]]) {
        const row = document.createElement("div");
        const term = document.createElement("dt"); term.textContent = label;
        const description = document.createElement("dd"); description.textContent = value; description.style.whiteSpace = "pre-wrap";
        row.append(term, description); list.append(row);
      }
      preview.querySelector("[data-presentation-size]").textContent = t("presentation.size", { session: input.session, bytes: UTF8_ENCODER.encode(validatedToml).length });
      preview.hidden = false;
      status.textContent = t("presentation.validated");
      preview.querySelector("h2").focus();
    } catch (error) {
      status.textContent = publicErrorMessage(error);
    } finally {
      busy = false;
      controls.forEach(control => { control.disabled = false; });
    }
  });
  download.addEventListener("click", () => {
    if (validatedToml === null) return;
    const url = URL.createObjectURL(new Blob([validatedToml], { type: "application/toml;charset=utf-8" }));
    const link = document.createElement("a"); link.href = url; link.download = "presentation.toml";
    document.body.append(link); link.click(); link.remove();
    window.setTimeout(() => URL.revokeObjectURL(url), 1000);
    preview.querySelector("[data-presentation-download-status]").textContent = t("presentation.downloadStarted");
  });
  form.hidden = false;
}
