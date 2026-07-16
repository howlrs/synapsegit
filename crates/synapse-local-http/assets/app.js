const TOKEN_HEADER = "X-Synapse-Local-Token";
const TOKEN_SELECTOR = 'meta[name="synapse-local-token"]';
const API_BASE_SELECTOR = 'meta[name="synapse-api-base"]';
const ENHANCED_FORMS = new WeakSet();
const ENHANCED_IMAGES = new WeakSet();
const CONTROL_DISABLED_STATE = new WeakMap();
const IMAGE_ELEMENTS = new Set();
const IMAGE_REQUESTS = new Map();
const IMAGE_URLS = new Map();
const ALLOWED_RASTER_TYPES = new Set(["image/png", "image/jpeg", "image/gif", "image/webp"]);
const ATTACHMENT_MEDIA_TYPE = "application/octet-stream";
const MAX_IMAGE_BYTES = 64 * 1024 * 1024;
const MAX_UPLOAD_AGGREGATE_BYTES = 3 * MAX_IMAGE_BYTES;
const CREATOR_TEXT_FIELDS = new Map([
  ["session", 64],
  ["subject_label", 500],
  ["creator_name", 300],
]);
const CREATOR_FILE_FIELDS = new Set(["original_image", "current_image", "ai_output"]);
const UTF8_ENCODER = new TextEncoder();

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

function operationSuccessMessage(operation) {
  if (operation?.state !== "succeeded") return null;
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
  return "Maintenance operation completed.";
}

async function pollOperation(form, accepted) {
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
    if (operation.state === "succeeded") return operation;
    if (operation.state === "failed" || operation.state === "outcome_unknown") {
      const detail =
        typeof operation.error?.detail === "string"
          ? operation.error.detail
          : "The maintenance operation did not complete successfully.";
      throw new TypeError(detail);
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

function installInlineRaster(image, objectUrl) {
  resetImageDownload(image);
  image.src = objectUrl;
  setImageStatus(image, "", null);
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
      headers: { Accept: [...ALLOWED_RASTER_TYPES, ATTACHMENT_MEDIA_TYPE].join(", ") },
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
    if (responseMode === "inline") installInlineRaster(image, objectUrl);
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
  for (const controller of IMAGE_REQUESTS.values()) controller.abort();
  IMAGE_REQUESTS.clear();

  for (const image of IMAGE_ELEMENTS) {
    revokeImageUrl(image);
    ENHANCED_IMAGES.delete(image);
  }
  IMAGE_ELEMENTS.clear();
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

  return payload;
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
  const text = new Map();
  const files = new Map();
  let aggregateBytes = 0;

  for (const [name, value] of source) {
    if (CREATOR_TEXT_FIELDS.has(name)) {
      if (typeof value !== "string" || text.has(name)) {
        throw new TypeError(`The field “${name}” must occur exactly once as text.`);
      }
      const byteLength = UTF8_ENCODER.encode(value).byteLength;
      if (byteLength === 0 || byteLength > CREATOR_TEXT_FIELDS.get(name)) {
        throw new TypeError(`The field “${name}” exceeds its UTF-8 byte limit.`);
      }
      text.set(name, value);
      continue;
    }

    if (CREATOR_FILE_FIELDS.has(name)) {
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

  if (text.size !== CREATOR_TEXT_FIELDS.size || files.size !== CREATOR_FILE_FIELDS.size) {
    throw new TypeError("The creator upload requires exactly three text fields and three files.");
  }
  if (!/^[a-z][a-z0-9-]{0,63}$/u.test(text.get("session"))) {
    throw new TypeError("The session field is not a valid lowercase slug.");
  }

  const normalized = new FormData();
  for (const name of CREATOR_TEXT_FIELDS.keys()) {
    normalized.append(
      name,
      new Blob([text.get(name)], { type: "text/plain; charset=utf-8" }),
      `${name}.txt`,
    );
  }
  for (const name of CREATOR_FILE_FIELDS) {
    normalized.append(
      name,
      new Blob([files.get(name)], { type: "application/octet-stream" }),
      `${name}.bin`,
    );
  }
  return normalized;
}

function formRequest(form, submitter) {
  const method = (submitter?.formMethod || form.method || "get").toUpperCase();
  const action = submitter?.formAction || form.action;
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

  return {
    url,
    init: {
      method,
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(formJson(form, submitter)),
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
  for (const control of form.querySelectorAll("[data-synapse-submit]")) {
    if (control instanceof HTMLButtonElement || control instanceof HTMLInputElement) {
      if (busy) {
        CONTROL_DISABLED_STATE.set(control, control.disabled);
        control.disabled = true;
      } else {
        control.disabled = CONTROL_DISABLED_STATE.get(control) ?? control.disabled;
        CONTROL_DISABLED_STATE.delete(control);
      }
    }
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

async function submitEnhancedForm(event) {
  const form = event.currentTarget;
  if (!(form instanceof HTMLFormElement)) return;

  event.preventDefault();
  if (!form.reportValidity() || form.getAttribute("aria-busy") === "true") return;

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
    const confirmed = window.confirm(
      `Creator session “${session}” に ${disposition} decisionを公開します。この操作を続けますか？`,
    );
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
    if (data?.state === "queued") {
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
        operationSuccessMessage(data) || form.dataset.successMessage || "Completed.",
        "success",
      );
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
    setStatus(form, publicErrorMessage(error), "error");
    form.dispatchEvent(
      new CustomEvent("synapse:api-error", {
        bubbles: true,
        detail: { error },
      }),
    );
  } finally {
    setBusy(form, false);
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
    form.addEventListener("submit", submitEnhancedForm);
    form.hidden = false;
  }
}

function start() {
  document.documentElement.classList.add("has-js");
  enhanceApiForms();
  enhanceApiImages();
}

if (document.readyState === "loading") {
  document.addEventListener("DOMContentLoaded", start, { once: true });
} else {
  start();
}

window.addEventListener("pagehide", releaseImageResources);
window.addEventListener("pageshow", (event) => {
  if (event.persisted) enhanceApiImages();
});
