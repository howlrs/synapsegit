#!/usr/bin/env node

import assert from "node:assert/strict";

const ORIGIN = "http://127.0.0.1:43123";

class FakeMetaElement {
  constructor(content) {
    this.content = content;
  }
}

class FakeElement {
  constructor() {
    this.attributes = new Map();
    this.dataset = {};
    this.hidden = false;
    this.textContent = "";
  }

  getAttribute(name) {
    return this.attributes.get(name) ?? null;
  }

  setAttribute(name, value) {
    this.attributes.set(name, String(value));
  }
}

class FakeForm extends FakeElement {
  constructor({ action, fields, dataset = {} }) {
    super();
    this.action = action;
    this.method = "post";
    this.fields = fields;
    this.elements = { namedItem: () => null };
    this.dataset = dataset;
    this.status = new FakeElement();
    this.hidden = true;
    this.listeners = new Map();
    this.controls = [];
  }

  addEventListener(name, listener) {
    this.listeners.set(name, listener);
  }

  dispatchEvent() {
    return true;
  }

  querySelector(selector) {
    return selector === "[data-synapse-status]" ? this.status : null;
  }

  querySelectorAll(selector) {
    if (selector === "[data-synapse-submit]") {
      return this.controls.filter((control) => control.getAttribute("data-synapse-submit") !== null);
    }
    if (selector === "input, button, select, textarea") return this.controls;
    return [];
  }

  reportValidity() {
    return true;
  }
}

class FakeButton extends FakeElement {
  constructor(form, { action = `${ORIGIN}/projects/demo`, explicitAction = false } = {}) {
    super();
    this.form = form;
    this.formAction = action;
    this.formMethod = "";
    this.name = "";
    this.value = "";
    this.disabled = false;
    if (explicitAction) this.setAttribute("formaction", action);
    form.controls.push(this);
  }

  hasAttribute(name) {
    return this.attributes.has(name);
  }
}

class FakeAnchor extends FakeElement {}

class FakeFormData {
  constructor(form) {
    this.entries = [...form.fields];
  }

  append(name, value) {
    this.entries.push([name, value]);
  }

  get(name) {
    return this.entries.find(([field]) => field === name)?.[1] ?? null;
  }

  [Symbol.iterator]() {
    return this.entries[Symbol.iterator]();
  }
}

globalThis.HTMLMetaElement = FakeMetaElement;
globalThis.HTMLElement = FakeElement;
globalThis.HTMLFormElement = FakeForm;
globalThis.HTMLButtonElement = FakeButton;
globalThis.HTMLInputElement = class extends FakeElement {};
globalThis.HTMLAnchorElement = FakeAnchor;
globalThis.FormData = FakeFormData;
globalThis.CustomEvent = class {
  constructor(name, options = {}) {
    this.type = name;
    this.detail = options.detail;
  }
};
globalThis.document = {
  readyState: "loading",
  documentElement: { classList: { add() {} } },
  addEventListener() {},
  querySelector(selector) {
    if (selector === 'meta[name="synapse-api-base"]') return new FakeMetaElement("/api/v1");
    if (selector === 'meta[name="synapse-local-token"]') return new FakeMetaElement("a".repeat(64));
    return null;
  },
};

let confirmPrompt = null;
let confirmationCount = 0;
let fetchCalled = false;
let confirmResult = false;
let locationAssigns = 0;
let locationReloads = 0;
globalThis.window = {
  addEventListener() {},
  confirm(prompt) {
    confirmationCount += 1;
    confirmPrompt = prompt;
    return confirmResult;
  },
  fetch() {
    fetchCalled = true;
    throw new Error("fetch must not run in a cancelled submission test");
  },
  location: {
    origin: ORIGIN,
    assign() {
      locationAssigns += 1;
    },
    reload() {
      locationReloads += 1;
    },
  },
  setTimeout(callback) {
    callback();
    return 1;
  },
};

const {
  enhanceApiForms,
  formRequest,
  operationSuccessMessage,
  submitEnhancedForm,
} = await import("../crates/synapse-local-http/assets/app.js");

const fields = [
  ["archive_name", "nightly-2026-08-27"],
  ["confirm_project_key", "demo"],
];
const form = new FakeForm({
  action: `${ORIGIN}/api/v1/projects/demo/archive-exports`,
  fields,
  dataset: { synapseApiForm: "json", confirmMaintenance: "archive-export" },
});

const ordinaryButton = new FakeButton(form);
const ordinaryRequest = formRequest(form, ordinaryButton);
assert.equal(ordinaryRequest.url.href, `${ORIGIN}/api/v1/projects/demo/archive-exports`);
assert.deepEqual(JSON.parse(ordinaryRequest.init.body), {
  archive_name: "nightly-2026-08-27",
  confirm_project_key: "demo",
});

const overrideButton = new FakeButton(form, {
  action: `${ORIGIN}/api/v1/projects/demo/operations/fsck`,
  explicitAction: true,
});
const overrideRequest = formRequest(form, overrideButton);
assert.equal(overrideRequest.url.href, `${ORIGIN}/api/v1/projects/demo/operations/fsck`);

let prevented = false;
await submitEnhancedForm({
  currentTarget: form,
  submitter: ordinaryButton,
  preventDefault() {
    prevented = true;
  },
});
assert.equal(prevented, true);
assert.equal(fetchCalled, false);
assert.match(confirmPrompt, /nightly-2026-08-27/u);
assert.equal(form.status.textContent, "Archive exportは開始されませんでした。");

const enhancedForm = new FakeForm({
  action: `${ORIGIN}/api/v1/projects/demo/archive-exports`,
  fields,
  dataset: { synapseApiForm: "json" },
});
enhanceApiForms({ querySelectorAll: () => [enhancedForm] });
assert.equal(enhancedForm.hidden, false);
assert.equal(enhancedForm.status.getAttribute("role"), "status");
assert.equal(enhancedForm.status.getAttribute("aria-live"), "polite");
assert.equal(enhancedForm.listeners.get("submit"), submitEnhancedForm);

assert.equal(
  operationSuccessMessage({
    state: "succeeded",
    kind: "archive_export",
    result: {
      archive_name: "nightly-2026-08-27",
      result_kind: "exported",
      report_equivalence_required: false,
    },
  }),
  "Archive “nightly-2026-08-27” was exported.",
);
assert.throws(
  () =>
    operationSuccessMessage({
      state: "succeeded",
      kind: "archive_export",
      result: {
        archive_name: "nightly-2026-08-27",
        result_kind: "unexpected",
        report_equivalence_required: false,
      },
    }),
  /archive-export result is invalid/u,
);

const restoreFields = [
  ["archive_name", "aaa-valid"],
  ["confirm_target_project_key", "demo"],
  ["confirm_empty_target", "true"],
];
function makeRestoreForm() {
  const restoreForm = new FakeForm({
    action: `${ORIGIN}/api/v1/projects/demo/archive-restores`,
    fields: restoreFields,
    dataset: {
      synapseApiForm: "json",
      archiveRestore: "true",
      restoreProjectKey: "demo",
    },
  });
  restoreForm.restoreLink = new FakeAnchor();
  const originalQuerySelector = restoreForm.querySelector.bind(restoreForm);
  restoreForm.querySelector = (selector) =>
    selector === "[data-synapse-restore-success-link]" ? restoreForm.restoreLink : originalQuerySelector(selector);
  return restoreForm;
}

const restoreForm = makeRestoreForm();

const restoreRequest = formRequest(restoreForm, new FakeButton(restoreForm));
assert.deepEqual(JSON.parse(restoreRequest.init.body), {
  archive_name: "aaa-valid",
  confirm_target_project_key: "demo",
  confirm_empty_target: true,
});

for (const invalidFields of [
  restoreFields.filter(([name]) => name !== "confirm_empty_target"),
  [
    ["archive_name", "aaa-valid"],
    ["confirm_target_project_key", "other"],
    ["confirm_empty_target", "true"],
  ],
  [
    ...restoreFields,
    ["confirm_empty_target", "true"],
  ],
  [...restoreFields, ["unknown", "value"]],
]) {
  const invalid = new FakeForm({
    action: `${ORIGIN}/api/v1/projects/demo/archive-restores`,
    fields: invalidFields,
    dataset: { synapseApiForm: "json", archiveRestore: "true", restoreProjectKey: "demo" },
  });
  assert.throws(() => formRequest(invalid, new FakeButton(invalid)), /archive restore confirmation/u);
}

confirmResult = false;
fetchCalled = false;
const cancelledRestoreForm = makeRestoreForm();
await submitEnhancedForm({
  currentTarget: cancelledRestoreForm,
  submitter: new FakeButton(cancelledRestoreForm),
  preventDefault() {},
});
assert.equal(fetchCalled, false);
assert.match(confirmPrompt, /aaa-valid/u);
assert.match(confirmPrompt, /demo/u);
assert.equal(cancelledRestoreForm.status.textContent, "Archive restoreは開始されませんでした。");
assert.equal(locationAssigns, 0);
assert.equal(locationReloads, 0);

const operationId = "a".repeat(22);
const capturedRequests = [];
const jsonResponse = (body) =>
  new Response(JSON.stringify(body), { headers: { "Content-Type": "application/json" } });
const successfulResponses = [
  jsonResponse({ state: "queued", operation_id: operationId, poll_path: `/api/v1/operations/${operationId}` }),
  jsonResponse({ operation_id: operationId, state: "queued" }),
  jsonResponse({ operation_id: operationId, state: "running" }),
  jsonResponse({
    operation_id: operationId,
    state: "succeeded",
    kind: "archive_restore",
    project_key: "demo",
    result: {
      archive_name: "aaa-valid",
      result_kind: "restored",
      report_equivalence_required: true,
    },
  }),
];
window.fetch = async (url, init) => {
  capturedRequests.push({ url: String(url), init });
  return successfulResponses.shift();
};
confirmResult = true;
const successfulRestoreForm = makeRestoreForm();
const successfulButton = new FakeButton(successfulRestoreForm);
successfulButton.setAttribute("data-synapse-submit", "");
const successfulInput = new HTMLInputElement();
successfulInput.disabled = false;
successfulRestoreForm.controls.push(successfulInput);
await submitEnhancedForm({
  currentTarget: successfulRestoreForm,
  submitter: successfulButton,
  preventDefault() {},
});
assert.deepEqual(JSON.parse(capturedRequests[0].init.body), {
  archive_name: "aaa-valid",
  confirm_target_project_key: "demo",
  confirm_empty_target: true,
});
assert.equal(capturedRequests[0].url, `${ORIGIN}/api/v1/projects/demo/archive-restores`);
assert.equal(capturedRequests[0].init.method, "POST");
assert.equal(capturedRequests[0].init.headers.get("X-Synapse-Local-Token"), "a".repeat(64));
assert.deepEqual(
  capturedRequests.slice(1).map(({ url, init }) => [url, init.method]),
  [
    [`${ORIGIN}/api/v1/operations/${operationId}`, "GET"],
    [`${ORIGIN}/api/v1/operations/${operationId}`, "GET"],
    [`${ORIGIN}/api/v1/operations/${operationId}`, "GET"],
  ],
);
assert.equal(successfulResponses.length, 0);
assert.equal(successfulRestoreForm.restoreLink.hidden, false);
assert.match(successfulRestoreForm.status.textContent, /creator-report/u);
assert.equal(successfulRestoreForm.getAttribute("aria-busy"), "false");
assert.equal(successfulButton.disabled, true);
assert.equal(successfulInput.disabled, true);
assert.equal(locationAssigns, 0);
assert.equal(locationReloads, 0);

const successfulStatus = successfulRestoreForm.status.textContent;
const successfulLink = successfulRestoreForm.restoreLink.hidden;
const requestCountAfterSuccess = capturedRequests.length;
const confirmationsAfterSuccess = confirmationCount;
await submitEnhancedForm({
  currentTarget: successfulRestoreForm,
  submitter: successfulButton,
  preventDefault() {},
});
assert.equal(capturedRequests.length, requestCountAfterSuccess);
assert.equal(confirmationCount, confirmationsAfterSuccess);
assert.equal(successfulRestoreForm.status.textContent, successfulStatus);
assert.equal(successfulRestoreForm.restoreLink.hidden, successfulLink);
assert.equal(successfulButton.disabled, true);
assert.equal(locationAssigns, 0);
assert.equal(locationReloads, 0);

for (const [terminal, expectedStatus] of [
  [{
    operation_id: operationId,
    state: "succeeded",
    kind: "archive_restore",
    project_key: "demo",
    result: { archive_name: "wrong", result_kind: "restored", report_equivalence_required: true },
  }, /archive-restore result is invalid/u],
  [{
    operation_id: operationId,
    state: "succeeded",
    kind: "archive_restore",
    project_key: "demo",
    result: { archive_name: "aaa-valid", result_kind: "restored", report_equivalence_required: false },
  }, /archive-restore result is invalid/u],
  [{
    operation_id: operationId,
    state: "succeeded",
    kind: "fsck",
    project_key: "demo",
    result: { clean: true, objects_verified: 0, issue_count: 0 },
  }, /maintenance operation status is invalid/u],
  [{
    operation_id: operationId,
    state: "succeeded",
    kind: "archive_restore",
    project_key: "other",
    result: { archive_name: "aaa-valid", result_kind: "restored", report_equivalence_required: true },
  }, /maintenance operation status is invalid/u],
  [{ operation_id: operationId, state: "failed", error: { detail: "restore failed" } }, /restore failed.*コピー済み/u],
  [{ operation_id: operationId, state: "outcome_unknown", error: { detail: "restore unknown" } }, /restore unknown.*コピー済み/u],
  [{
    operation_id: "b".repeat(22),
    state: "succeeded",
    kind: "archive_restore",
    project_key: "demo",
    result: { archive_name: "aaa-valid", result_kind: "restored", report_equivalence_required: true },
  }, /maintenance operation status is invalid/u],
]) {
  const failingForm = makeRestoreForm();
  failingForm.restoreLink.hidden = false;
  const sequence = [
    jsonResponse({ state: "queued", operation_id: operationId, poll_path: `/api/v1/operations/${operationId}` }),
    jsonResponse(terminal),
  ];
  window.fetch = async () => sequence.shift();
  await submitEnhancedForm({
    currentTarget: failingForm,
    submitter: new FakeButton(failingForm),
    preventDefault() {},
  });
  assert.equal(failingForm.restoreLink.hidden, true);
  assert.equal(failingForm.status.dataset.tone, "error");
  assert.match(failingForm.status.textContent, expectedStatus);
  assert.equal(failingForm.getAttribute("aria-busy"), "false");
  assert.equal(locationAssigns, 0);
  assert.equal(locationReloads, 0);
}

for (const initial of [
  {},
  {
    state: "succeeded",
    kind: "archive_restore",
    project_key: "demo",
    result: { archive_name: "aaa-valid", result_kind: "restored", report_equivalence_required: true },
  },
  { state: "succeeded", kind: "fsck", project_key: "demo", result: {} },
]) {
  const invalidReceiptForm = makeRestoreForm();
  invalidReceiptForm.restoreLink.hidden = false;
  window.fetch = async () => jsonResponse(initial);
  await submitEnhancedForm({
    currentTarget: invalidReceiptForm,
    submitter: new FakeButton(invalidReceiptForm),
    preventDefault() {},
  });
  assert.equal(invalidReceiptForm.restoreLink.hidden, true);
  assert.match(invalidReceiptForm.status.textContent, /archive restore receipt is invalid/u);
  assert.equal(invalidReceiptForm.status.dataset.tone, "error");
  assert.equal(locationAssigns, 0);
  assert.equal(locationReloads, 0);
}

console.log("local_app_ok: form_action json cancel live_region archive_result archive_restore");
