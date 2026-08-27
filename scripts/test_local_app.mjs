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

  querySelectorAll() {
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
  }

  hasAttribute(name) {
    return this.attributes.has(name);
  }
}

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
globalThis.FormData = FakeFormData;
globalThis.document = {
  readyState: "loading",
  documentElement: { classList: { add() {} } },
  addEventListener() {},
  querySelector(selector) {
    if (selector === 'meta[name="synapse-api-base"]') return new FakeMetaElement("/api/v1");
    return null;
  },
};

let confirmPrompt = null;
let fetchCalled = false;
globalThis.window = {
  addEventListener() {},
  confirm(prompt) {
    confirmPrompt = prompt;
    return false;
  },
  fetch() {
    fetchCalled = true;
    throw new Error("fetch must not run in a cancelled submission test");
  },
  location: {
    origin: ORIGIN,
    assign() {},
    reload() {},
  },
  setTimeout,
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

console.log("local_app_ok: form_action json cancel live_region archive_result");
