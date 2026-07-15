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
const MAX_IMAGE_BYTES = 64 * 1024 * 1024;

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

function revokeImageUrl(image) {
  const objectUrl = IMAGE_URLS.get(image);
  if (!objectUrl) return;
  URL.revokeObjectURL(objectUrl);
  IMAGE_URLS.delete(image);
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
  image.setAttribute("aria-busy", "true");
  setImageStatus(image, image.dataset.loadingMessage || "Loading image…", null);

  try {
    const response = await apiFetch(source, {
      headers: { Accept: [...ALLOWED_RASTER_TYPES].join(", ") },
      signal: controller.signal,
    });
    if (!response.ok) throw await responseProblem(response);

    const type = response.headers.get("Content-Type")?.split(";", 1)[0].trim().toLowerCase();
    const disposition = response.headers.get("Content-Disposition")?.trim().toLowerCase() || "";
    if (!type || !ALLOWED_RASTER_TYPES.has(type) || disposition.startsWith("attachment")) {
      throw new TypeError("The response is not an inline-safe raster image.");
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
    image.src = objectUrl;
    setImageStatus(image, "", null);
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
  const data = submitter ? new FormData(form, submitter) : new FormData(form);

  for (const [name, value] of data) {
    if (value instanceof File) {
      throw new TypeError("File forms require the dedicated upload enhancement.");
    }
    if (Object.hasOwn(payload, name)) {
      throw new TypeError(`The field “${name}” must occur exactly once.`);
    }
    payload[name] = value;
  }

  return payload;
}

function formRequest(form, submitter) {
  const method = (submitter?.formMethod || form.method || "get").toUpperCase();
  const action = submitter?.formAction || form.action;
  const url = resolveApiUrl(action);

  if (method === "GET") {
    const fields = submitter ? new FormData(form, submitter) : new FormData(form);
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

  if (form.dataset.synapseApiForm !== "json") {
    throw new TypeError("Only JSON API forms are enhanced by the shared module.");
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

async function submitEnhancedForm(event) {
  const form = event.currentTarget;
  if (!(form instanceof HTMLFormElement)) return;

  event.preventDefault();
  if (!form.reportValidity() || form.getAttribute("aria-busy") === "true") return;

  setBusy(form, true);
  setStatus(form, form.dataset.busyMessage || "Working…", null);

  try {
    const destination = form.dataset.successLocation
      ? new URL(form.dataset.successLocation, window.location.origin)
      : null;
    if (destination && destination.origin !== window.location.origin) {
      throw new TypeError("The success destination must use the local application origin.");
    }

    const { url, init } = formRequest(form, event.submitter);
    const data = await apiJson(url, init);
    setStatus(form, form.dataset.successMessage || "Completed.", "success");

    form.dispatchEvent(
      new CustomEvent("synapse:api-success", {
        bubbles: true,
        detail: { data },
      }),
    );

    if (destination) {
      window.location.assign(destination);
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
