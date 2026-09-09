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

let imageComparison;
let localToken;
let apiBase;

export class SynapseApiError extends Error {
  constructor(problem, response) {
    const detail = typeof problem?.detail === "string" ? problem.detail : null;
    const title = typeof problem?.title === "string" ? problem.title : null;
    super(detail || title || `Request failed with status ${response.status}`);
    this.name = "SynapseApiError";
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
    throw new Error("The local browser session token is missing or invalid.");
  }

  localToken = token;
  return localToken;
}

function getApiBase() {
  if (apiBase !== undefined) return apiBase;

  const configured = readMetaContent(API_BASE_SELECTOR) || "/api/v1";
  const resolved = new URL(configured, window.location.origin);
  if (resolved.origin !== window.location.origin || resolved.search || resolved.hash) {
    throw new Error("The configured API base must be a same-origin path.");
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
    throw new TypeError("API requests must stay within the same-origin API base.");
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
    throw new TypeError("The archive-restore result is invalid.");
  }
  if (operation.kind === "fsck") {
    const result = operation.result;
    if (
      !result ||
      typeof result.clean !== "boolean" ||
      !Number.isSafeInteger(result.objects_verified) ||
      !Number.isSafeInteger(result.issue_count)
    ) {
      throw new TypeError("The integrity-check result is invalid.");
    }
    return result.clean
      ? `Integrity check completed cleanly after verifying ${result.objects_verified} objects.`
      : `Integrity check completed with ${result.issue_count} issues.`;
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
      throw new TypeError("The archive-export result is invalid.");
    }
    return `Archive “${result.archive_name}” was exported.`;
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
      throw new TypeError("The archive-restore result is invalid.");
    }
    return `アーカイブ「${result.archive_name}」を復元しました。復元した履歴を利用する前に、保存元と復元先の creator-report を比較し、一致を確認して保管してください。`;
  }
  return "Maintenance operation completed.";
}

async function pollOperation(form, accepted, expectedOperation) {
  if (!validOperationAccepted(accepted)) {
    throw new TypeError("The maintenance operation receipt is invalid.");
  }
  const pollUrl = resolveApiUrl(accepted.poll_path);
  let pollDelayMs = 250;
  for (;;) {
    const operation = await apiJson(pollUrl, { method: "GET" });
    if (operation?.operation_id !== accepted.operation_id || typeof operation.state !== "string") {
      throw new TypeError("The maintenance operation status is invalid.");
    }
    if (operation.state === "queued" || operation.state === "running") {
      setStatus(form, `Maintenance operation is ${operation.state}…`, null);
      await new Promise((resolve) => window.setTimeout(resolve, pollDelayMs));
      pollDelayMs = Math.min(pollDelayMs * 2, 2_000);
      continue;
    }
    if (operation.state === "succeeded") {
      if (
        expectedOperation &&
        (operation.kind !== expectedOperation.kind || operation.project_key !== expectedOperation.projectKey)
      ) {
        throw new TypeError("The maintenance operation status is invalid.");
      }
      return operation;
    }
    if (operation.state === "failed" || operation.state === "outcome_unknown") {
      const detail =
        typeof operation.error?.detail === "string"
          ? operation.error.detail
          : "The maintenance operation did not complete successfully.";
      throw new TypeError(
        expectedOperation?.kind === "archive_restore"
          ? `${detail} ファイルの一部だけがコピー済みの可能性があります。確認後も同じアーカイブだけを再試行してください。`
          : detail,
      );
    }
    throw new TypeError("The maintenance operation status is invalid.");
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
    throw new TypeError("画像を表示できません。画像データが破損しているか、ブラウザーが対応していません。");
  }
  if (IMAGE_URLS.get(image) !== objectUrl) return;
  INLINE_IMAGES.add(image);
  refreshPinEditors();
  setImageStatus(image, `${image.naturalWidth} × ${image.naturalHeight} px`, null);
}

function installAttachmentDownload(image, objectUrl) {
  const download = imageDownloadElement(image);
  if (!download || !download.hasAttribute("download")) {
    throw new TypeError("The attachment-only response has no dedicated download action.");
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
      setImageStatus(image, "Download済みです。再取得する場合はページを再読み込みしてください。", null);
    }, 0);
  };
  setImageStatus(
    image,
    "Inline表示できない形式です。検証済みraw bytesをdownloadして確認してください。",
    "warning",
  );
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
    setImageStatus(image, "Image source unavailable.", "error");
    return;
  }

  const controller = new AbortController();
  IMAGE_REQUESTS.set(image, controller);
  revokeImageUrl(image);
  image.removeAttribute("src");
  image.setAttribute("aria-busy", "true");
  setImageStatus(image, image.dataset.loadingMessage || "Loading image…", null);

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
      throw new TypeError("The response is neither an inline-safe raster nor a safe attachment.");
    }

    const contentLength = response.headers.get("Content-Length");
    if (contentLength) {
      if (!/^\d+$/u.test(contentLength) || BigInt(contentLength) > BigInt(MAX_IMAGE_BYTES)) {
        throw new TypeError("The image exceeds the 64 MiB display limit.");
      }
    }

    const blob = await response.blob();
    if (blob.size > MAX_IMAGE_BYTES) {
      throw new TypeError("The image exceeds the 64 MiB display limit.");
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

/** Compare decoded inline rasters; attachment Blob URLs never enter this view. */
export function enhanceImageComparison(root = document) {
  const section = root.querySelector("[data-synapse-comparison]");
  if (!section || imageComparison || typeof HTMLDialogElement === "undefined") return;
  const dialog = section.querySelector("dialog");
  if (!(dialog instanceof HTMLDialogElement) || typeof dialog.showModal !== "function") return;
  const opener = section.querySelector("[data-synapse-compare-open]");
  const status = section.querySelector("[data-synapse-compare-status]");
  const zoom = dialog.querySelector("[data-synapse-compare-zoom]");
  const sources = [...section.querySelectorAll("img[data-synapse-image]")];
  const panes = [...dialog.querySelectorAll("[data-synapse-compare-pane]")].map((pane) => ({
    select: pane.querySelector("[data-synapse-compare-source]"),
    image: pane.querySelector("[data-synapse-compare-image]"),
    caption: pane.querySelector("[data-synapse-compare-caption]"),
    viewport: pane.querySelector("[data-synapse-compare-viewport]"),
  }));
  const ready = (source) => source && INLINE_IMAGES.has(source) && IMAGE_URLS.has(source);
  const clear = () => {
    for (const pane of panes) {
      pane.image.removeAttribute("src");
      pane.image.hidden = true;
      pane.image.alt = "";
      pane.caption.textContent = "";
    }
  };
  const render = () => {
    if (!dialog.open) return;
    for (const pane of panes) {
      const source = sources[Number(pane.select.value)];
      if (!ready(source)) {
        pane.image.removeAttribute("src");
        pane.image.hidden = true;
        pane.caption.textContent = "この画像は表示できません。元のカードの状態を確認してください。";
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
  };
  const refresh = () => {
    const count = sources.filter(ready).length;
    opener.disabled = count < 2;
    status.textContent = count >= 2
      ? "表示できる画像を2枚選んで、全体表示・100%・200%で確認できます。"
      : "比較には表示可能な画像が2枚必要です。読み込み中・表示不可の理由は各カードで確認できます。";
    for (const pane of panes) {
      for (const option of pane.select.options) option.disabled = !ready(sources[Number(option.value)]);
    }
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
    dialog.showModal();
    render();
    for (const pane of panes) pane.viewport.scrollTo(0, 0);
  });
  dialog.querySelector("[data-synapse-compare-close]").addEventListener("click", () => dialog.close());
  dialog.addEventListener("close", () => {
    clear();
    opener.focus({ preventScroll: true });
  });
  for (const pane of panes) {
    pane.select.addEventListener("change", () => {
      render();
      pane.viewport.scrollTo(0, 0);
    });
  }
  zoom.addEventListener("change", () => {
    render();
    for (const pane of panes) pane.viewport.scrollTo(0, 0);
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
  const error = count > limit ? `${limit} bytes以内に短くしてください（現在 ${count} bytes）。` : "";
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
      target.textContent = `選択済み ${files.length} / ${fields.length} ファイル · 合計 ${fileSizeLabel(files.reduce((total, file) => total + file.size, 0))} / ${fields.length * 64} MiB`;
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
      const error = file?.size > MAX_IMAGE_BYTES ? "64 MiB以内のファイルを選び直してください。" : "";
      field.input.setCustomValidity(error);
      field.input.setAttribute("aria-invalid", String(Boolean(error)));
      summary();
      if (!file) return;
      if (error) {
        field.status.textContent = error;
        field.status.dataset.tone = "error";
        return;
      }
      field.status.textContent = "取り込み前のプレビューを読み込んでいます…";
      try {
        // Read only the signature before allocating a URL. File.type and the
        // filename are caller supplied and never authorize inline rendering.
        const type = localPreviewMediaType(new Uint8Array(await file.slice(0, 12).arrayBuffer()));
        if (!isCurrent()) return;
        if (!type) {
          field.status.textContent = "この形式はプレビューできません。ファイルはそのまま取り込めます。";
          return;
        }
        state.url = URL.createObjectURL(file.slice(0, file.size, type));
        field.image.src = state.url;
        await field.image.decode();
        if (!isCurrent()) return;
        field.image.hidden = false;
        field.status.textContent = `${field.image.naturalWidth} × ${field.image.naturalHeight} px · ローカルプレビュー`;
      } catch {
        if (!isCurrent()) return;
        release(field);
        field.status.textContent = "プレビューを表示できません。ファイルの内容を確認してください。そのまま取り込むこともできます。";
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
      throw new TypeError("File forms require the dedicated upload enhancement.");
    }
    if (Object.hasOwn(payload, name)) {
      throw new TypeError(`The field “${name}” must occur exactly once.`);
    }
    const control = form.elements.namedItem(name);
    if (control instanceof HTMLElement && control.dataset.maxUtf8Bytes) {
      const limit = Number(control.dataset.maxUtf8Bytes);
      if (!Number.isSafeInteger(limit) || limit < 0 || UTF8_ENCODER.encode(value).byteLength > limit) {
        throw new TypeError(`The field “${name}” exceeds its UTF-8 byte limit.`);
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
    if (UTF8_ENCODER.encode(JSON.stringify(payload)).byteLength > 8192) throw new TypeError("The decision JSON exceeds 8 KiB including rationale and pins.");
  }
  return payload;
}

function archiveRestoreJson(form, submitter) {
  const expectedProjectKey = form.dataset.restoreProjectKey;
  if (typeof expectedProjectKey !== "string" || !/^[a-z][a-z0-9-]{0,63}$/u.test(expectedProjectKey)) {
    throw new TypeError("The restore target project is invalid.");
  }

  const fields = new Map();
  for (const [name, value] of formDataWithSubmitter(form, submitter)) {
    if (
      value instanceof File ||
      !["archive_name", "confirm_target_project_key", "confirm_empty_target"].includes(name) ||
      fields.has(name)
    ) {
      throw new TypeError("The archive restore confirmation is invalid.");
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
    throw new TypeError("The archive restore confirmation is invalid.");
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
      throw new TypeError("The submit control does not belong to this form.");
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
        throw new TypeError(`The field “${name}” must occur exactly once as text.`);
      }
      const byteLength = UTF8_ENCODER.encode(value).byteLength;
      if ((!name.startsWith("generation_") && byteLength === 0) || byteLength > textFields.get(name)) {
        throw new TypeError(`The field “${name}” exceeds its UTF-8 byte limit.`);
      }
      text.set(name, value);
      continue;
    }

    if (fileFields.has(name)) {
      if (!(value instanceof File) || files.has(name)) {
        throw new TypeError(`The field “${name}” must occur exactly once as a file.`);
      }
      if (value.size > MAX_IMAGE_BYTES) {
        throw new TypeError(`The file “${name}” exceeds the 64 MiB limit.`);
      }
      aggregateBytes += value.size;
      if (aggregateBytes > MAX_UPLOAD_AGGREGATE_BYTES) {
        throw new TypeError("The three files exceed the 192 MiB aggregate limit.");
      }
      files.set(name, value);
      continue;
    }

    throw new TypeError(`The field “${name}” is not allowed in a creator upload.`);
  }

  const note = Object.fromEntries(["tool", "model", "prompt", "intent"].map(key => [key, text.get(`generation_${key}`) || ""]));
  if (UTF8_ENCODER.encode(JSON.stringify(note)).byteLength > 16384) throw new TypeError("生成メモ全体は16 KiB以内にしてください。");
  if (!["session", "subject_label", "creator_name"].every(name => text.has(name)) || files.size !== fileFields.size) {
    throw new TypeError("The creator upload contains missing or unexpected fields.");
  }
  if (!/^[a-z][a-z0-9-]{0,63}$/u.test(text.get("session"))) {
    throw new TypeError("The session field is not a valid lowercase slug.");
  }

  if (derived && !/^[0-9a-f]{64}$/u.test(text.get("confirmation_id") || "")) throw new TypeError("The source confirmation is invalid. Reopen the source page.");
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
      if (typeof value !== "string") throw new TypeError("GET forms cannot contain files.");
      if (fieldNames.has(name)) {
        throw new TypeError(`The field “${name}” must occur exactly once.`);
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
    throw new TypeError("The API form enhancement is not recognized.");
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

function publicErrorMessage(error) {
  if (error instanceof SynapseApiError) return error.message;
  if (error instanceof TypeError) return error.message;
  if (error instanceof DOMException && error.name === "AbortError") return "Request cancelled.";
  return "The local application could not complete the request.";
}

function showCommittedReceipt(form, data) {
  const receipt = data?.receipt;
  if (!receipt || typeof receipt !== "object" || typeof receipt.decision_head !== "string") {
    throw new TypeError("The committed creator receipt is invalid.");
  }

  setStatus(
    form,
    `Decision committed at ${receipt.decision_head}. The full report is unavailable; inspect and retain the durable receipt below.`,
    "success",
  );
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
    const session = form.dataset.confirmSession || "this session";
    const project = form.dataset.confirmProject;
    const summary = event.submitter.dataset.decisionSummary;
    const confirmed = window.confirm([
      `${project ? `Project “${project}” / ` : ""}Creator session “${session}” に ${disposition} decisionを公開します。`,
      ...(summary ? [summary, "記録後にこのセッションの判断を変更・再開する機能はありません。"] : []),
      "この操作を続けますか？",
    ].join("\n"));
    if (!confirmed) {
      setStatus(form, "Decisionは送信されませんでした。", null);
      return;
    }
  }

  if (form.dataset.confirmMaintenance === "fsck") {
    const confirmed = window.confirm(
      "Read-only fsckを開始します。大きなrepositoryでは完了まで時間がかかる場合があります。続行しますか？",
    );
    if (!confirmed) {
      setStatus(form, "Integrity checkは開始されませんでした。", null);
      return;
    }
  }

  if (form.dataset.confirmMaintenance === "archive-export") {
    const archiveName = new FormData(form).get("archive_name");
    if (typeof archiveName !== "string" || !/^[a-z][a-z0-9-]{0,63}$/u.test(archiveName)) {
      setStatus(form, "Archive exportは開始されませんでした。", null);
      return;
    }
    const confirmed = window.confirm(
      `新しいarchive “${archiveName}” を作成します。既存archiveは上書きされません。続行しますか？`,
    );
    if (!confirmed) {
      setStatus(form, "Archive exportは開始されませんでした。", null);
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
      `archive “${archiveRestore.archiveName}” をtarget project “${archiveRestore.projectKey}” へ復元します。失敗時もファイルの一部がコピー済みの可能性があり、自動再試行は行いません。続行しますか？`,
    );
    if (!confirmed) {
      setStatus(form, "Archive restoreは開始されませんでした。", null);
      return;
    }
  }

  setBusy(form, true);
  setStatus(form, form.dataset.busyMessage || "Working…", null);

  try {
    let destination = form.dataset.successLocation
      ? new URL(form.dataset.successLocation, window.location.origin)
      : null;
    if (destination && destination.origin !== window.location.origin) {
      throw new TypeError("The success destination must use the local application origin.");
    }

    let data = await apiJson(prepared.url, prepared.init);
    if (archiveRestore) {
      if (!validOperationAccepted(data)) {
        throw new TypeError("The archive restore receipt is invalid.");
      }
      setStatus(form, "Archive restoreを開始しました…", null);
      data = await pollOperation(form, data, archiveRestore);
    } else if (data?.state === "queued") {
      setStatus(form, "Maintenance operationを開始しました…", null);
      data = await pollOperation(form, data);
    }
    const committedWithoutReport = data?.state === "committed";
    if (committedWithoutReport) {
      showCommittedReceipt(form, data);
      destination = null;
    } else {
      setStatus(
        form,
        operationSuccessMessage(data, archiveRestore) || form.dataset.successMessage || "Completed.",
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
        throw new TypeError("The creator session destination is invalid.");
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
      status.textContent = "注釈を表示できません。未知の形式または不正な画像対応です。";
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
        throw new TypeError("ピンの画像・座標と、各メモが1〜200 UTF-8 bytesであることを確認してください。");
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
        remaining.push(`${button.textContent}: 残り ${8192 - bytes} bytes`);
        button.disabled = busy || invalidPins || bytes > 8192;
      }
      counter.textContent = `送信JSON全体（上限8192 bytes、理由とピンを含む） · ${remaining.join(" / ")}${invalidPins ? " · ピンのメモ・座標を修正してください。" : ""}`;
    }
    function positionMarkers() {
      for (const marker of markers.children) {
        const pin = pins[Number(marker.dataset.pinIndex)];
        marker.style.left = `${pin.x / PIN_MAX * 100}%`;
        marker.style.top = `${pin.y / PIN_MAX * 100}%`;
        marker.setAttribute("aria-label", `ピン ${Number(marker.dataset.pinIndex) + 1}: ${pin.note || "メモ未入力"}`);
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
        show.textContent = `ピン ${index + 1} · ${pin.role}`;
        show.addEventListener("click", () => { selector.value = pin.role; refresh(); markers.querySelector(`[data-pin-index="${index}"]`)?.focus(); });
        item.append(show);
        if (editable) {
          const label = document.createElement("label"); label.textContent = `ピン ${index + 1} のメモ`;
          const text = document.createElement("textarea"); text.id = `pin-note-${index}`; text.rows = 2; text.value = pin.note; label.htmlFor = text.id;
          const count = document.createElement("span"); count.className = "field__hint";
          const updateNote = () => { pin.note = text.value; const bytes = UTF8_ENCODER.encode(pin.note).byteLength; count.textContent = `${bytes} / 200 bytes`; text.setAttribute("aria-invalid", String(bytes === 0 || bytes > 200)); positionMarkers(); updateJsonCount(); };
          text.addEventListener("input", updateNote);
          item.append(label, text, count);
          for (const axis of ["x", "y"]) {
            const axisLabel = document.createElement("label"); axisLabel.textContent = `ピン ${index + 1} ${axis.toUpperCase()}座標`;
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
          const deletion = document.createElement("button"); deletion.type = "button"; deletion.textContent = `ピン ${index + 1} を削除`; deletion.dataset.variant = "secondary";
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
        status.textContent = `${source.dataset.label} · ${source.naturalWidth} × ${source.naturalHeight} px · 画像の向きを反映した表示`;
      } else {
        preview.removeAttribute("src");
        status.textContent = "この画像にはピンを作成・表示できません。読み込み中、attachment扱い、または画像のdecode失敗です。元画像の状態を確認してください。";
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

function start() {
  document.documentElement.classList.add("has-js");
  enhancePresentationForm();
  enhanceCreatorPins();
  enhanceApiForms();
  enhanceCreatorUploads();
  enhanceImageComparison();
  enhanceApiImages();
}

if (document.readyState === "loading") {
  document.addEventListener("DOMContentLoaded", start, { once: true });
} else {
  start();
}

window.addEventListener("pagehide", releaseImageResources);
window.addEventListener("pagehide", releaseCreatorPreviews);
window.addEventListener("pageshow", (event) => {
  if (event.persisted) {
    enhanceApiImages();
    for (const upload of CREATOR_UPLOADS.values()) upload.refresh();
  }
});

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
        const message = `${control.labels[0].textContent}: ${bytes} / ${limit} UTF-8 bytes。上限を超えています。`;
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
    status.textContent = "文章を検証しています…";
    try {
      const result = await apiJson(form.dataset.endpoint, { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(input) });
      if (revision !== currentRevision) return;
      if (typeof result?.toml !== "string" || UTF8_ENCODER.encode(result.toml).length > 65536) throw new Error("説明文ファイルの応答が不正です。");
      validatedToml = result.toml;
      const list = preview.querySelector("[data-presentation-text]");
      for (const [label, value] of entries.length ? entries : [["説明文", "未入力。既存の省略時動作を使用します。"]]) {
        const row = document.createElement("div");
        const term = document.createElement("dt"); term.textContent = label;
        const description = document.createElement("dd"); description.textContent = value; description.style.whiteSpace = "pre-wrap";
        row.append(term, description); list.append(row);
      }
      preview.querySelector("[data-presentation-size]").textContent = `対象: ${input.session} · presentation.toml: ${UTF8_ENCODER.encode(validatedToml).length} / 65536 bytes`;
      preview.hidden = false;
      status.textContent = "検証できました。内容を確認して書き出してください。";
      preview.querySelector("h2").focus();
    } catch (error) {
      status.textContent = error instanceof Error ? error.message : "文章の検証に失敗しました。";
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
    preview.querySelector("[data-presentation-download-status]").textContent = "説明文ファイルのダウンロードを開始しました。bundle生成・検証と外部共有はまだ行っていません。";
  });
  form.hidden = false;
}
