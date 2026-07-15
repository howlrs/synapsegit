#!/usr/bin/env node

import fs from "node:fs";
import path from "node:path";
import process from "node:process";

const contractPath = path.join(process.cwd(), "api", "local", "v1", "openapi.json");
const failures = [];

let contract;
try {
  contract = JSON.parse(fs.readFileSync(contractPath, "utf8"));
} catch (error) {
  console.error("local_api_error: cannot parse " + contractPath + ": " + error.message);
  process.exit(1);
}

function fail(message) {
  failures.push(message);
}

function decodePointerToken(token) {
  return token.replaceAll("~1", "/").replaceAll("~0", "~");
}

function resolveReference(reference) {
  if (!reference.startsWith("#/")) {
    fail("external reference is not allowed: " + reference);
    return undefined;
  }
  let value = contract;
  for (const token of reference.slice(2).split("/")) {
    const key = decodePointerToken(token);
    if (value === null || typeof value !== "object" || !(key in value)) {
      fail("unresolved reference: " + reference);
      return undefined;
    }
    value = value[key];
  }
  return value;
}

function walk(value, visit, location = "#", seen = new Set()) {
  if (value === null || typeof value !== "object") {
    return;
  }
  if (seen.has(value)) {
    return;
  }
  seen.add(value);
  visit(value, location);
  if (Array.isArray(value)) {
    value.forEach((entry, index) => walk(entry, visit, location + "/" + index, seen));
    return;
  }
  for (const [key, entry] of Object.entries(value)) {
    walk(entry, visit, location + "/" + key, seen);
  }
}

if (contract.openapi !== "3.1.1") {
  fail("openapi must be exactly 3.1.1");
}
if (contract["x-synapse-status"] !== "partially-implemented-contract") {
  fail("x-synapse-status must state partially-implemented-contract");
}

const servers = contract.servers ?? [];
if (
  servers.length !== 1 ||
  servers[0]?.url !== "http://127.0.0.1:{port}/api/v1"
) {
  fail("the contract must expose exactly one canonical IPv4-loopback server URL");
}

function isLocalTokenOnly(security) {
  return (
    Array.isArray(security) &&
    security.length === 1 &&
    security[0] !== null &&
    typeof security[0] === "object" &&
    Object.keys(security[0]).length === 1 &&
    Array.isArray(security[0].localSessionToken) &&
    security[0].localSessionToken.length === 0
  );
}

if (!isLocalTokenOnly(contract.security)) {
  fail("global security must be exactly the custom local session token");
}
const localToken = contract.components?.securitySchemes?.localSessionToken;
if (
  localToken?.type !== "apiKey" ||
  localToken?.in !== "header" ||
  localToken?.name !== "X-Synapse-Local-Token"
) {
  fail("localSessionToken must be the exact X-Synapse-Local-Token apiKey header");
}

let referenceCount = 0;
walk(contract, (value) => {
  if (typeof value.$ref === "string") {
    referenceCount += 1;
    resolveReference(value.$ref);
  }
});

const httpMethods = new Set(["get", "put", "post", "delete", "options", "head", "patch", "trace"]);
const operationIds = new Set();
const operations = [];
const expectedOperations = new Map([
  ["GET /health", ["getHealth", 2]],
  ["GET /projects", ["listProjects", 2]],
  ["GET /projects/{projectKey}/status", ["getProjectStatus", 2]],
  ["GET /projects/{projectKey}/refs", ["listProjectRefs", 2]],
  ["GET /projects/{projectKey}/reflog", ["listProjectReflog", 2]],
  ["GET /projects/{projectKey}/creator-sessions", ["listCreatorSessions", 2]],
  ["POST /projects/{projectKey}/creator-sessions", ["beginCreatorSession", 4]],
  ["GET /projects/{projectKey}/creator-sessions/{session}", ["getCreatorSession", 2]],
  [
    "GET /projects/{projectKey}/creator-sessions/{session}/images/{role}",
    ["getCreatorSessionImage", 2],
  ],
  [
    "POST /projects/{projectKey}/creator-sessions/{session}/decisions",
    ["decideCreatorSession", 6],
  ],
  [
    "GET /projects/{projectKey}/creator-sessions/{session}/diagnostics",
    ["getCreatorSessionDiagnostics", 8],
  ],
  ["POST /projects/{projectKey}/operations/fsck", ["startFsck", 7]],
  ["POST /projects/{projectKey}/archive-exports", ["startArchiveExport", 7]],
  ["POST /projects/{projectKey}/archive-restores", ["startArchiveRestore", 7]],
  ["GET /archives", ["listArchives", 7]],
  ["GET /operations/{operationId}", ["getOperation", 7]],
]);
const missingOperations = new Set(expectedOperations.keys());
const forbiddenPathFragments = [
  "update-ref",
  "update_ref",
  "/objects",
  "/commits",
  "/authority",
  "/profiles",
  "/permits",
];

for (const [route, pathItem] of Object.entries(contract.paths ?? {})) {
  if (!route.startsWith("/")) {
    fail("route must start with /: " + route);
  }
  for (const fragment of forbiddenPathFragments) {
    if (route.includes(fragment)) {
      fail("forbidden low-level route capability: " + route);
    }
  }

  for (const [method, operation] of Object.entries(pathItem)) {
    if (!httpMethods.has(method)) {
      continue;
    }
    operations.push({
      route,
      method,
      operation,
      parameters: [...(pathItem.parameters ?? []), ...(operation.parameters ?? [])],
    });

    const operationKey = method.toUpperCase() + " " + route;
    const expected = expectedOperations.get(operationKey);
    if (!expected) {
      fail("unexpected route operation: " + operationKey);
    } else {
      missingOperations.delete(operationKey);
      if (operation.operationId !== expected[0] || operation["x-synapse-implementation-slice"] !== expected[1]) {
        fail(operationKey + " does not match its frozen operationId/slice");
      }
    }

    if (typeof operation.operationId !== "string" || operation.operationId.length === 0) {
      fail(method.toUpperCase() + " " + route + " has no operationId");
    } else if (operationIds.has(operation.operationId)) {
      fail("duplicate operationId: " + operation.operationId);
    } else {
      operationIds.add(operation.operationId);
    }

    const slice = operation["x-synapse-implementation-slice"];
    if (!Number.isInteger(slice) || slice < 2 || slice > 8) {
      fail(method.toUpperCase() + " " + route + " has an invalid implementation slice");
    }

    const isHealth = method === "get" && route === "/health";
    const effectiveSecurity = operation.security ?? contract.security;
    if (isHealth) {
      if (!Array.isArray(operation.security) || operation.security.length !== 0) {
        fail("GET /health must explicitly opt out of the local session token");
      }
    } else if (!isLocalTokenOnly(effectiveSecurity)) {
      fail(method.toUpperCase() + " " + route + " must require only localSessionToken");
    }

    if (method !== "get" && method !== "head" && method !== "options") {
      if (method !== "post") {
        fail("the first contract permits state-changing POST only: " + method.toUpperCase() + " " + route);
      }
      if (!operation.requestBody) {
        fail(method.toUpperCase() + " " + route + " must have a request body");
      }
    }

    if (!isHealth && !Object.hasOwn(operation.responses ?? {}, "default")) {
      fail(method.toUpperCase() + " " + route + " must define the shared Problem response");
    }
  }
}

for (const operationKey of missingOperations) {
  fail("missing frozen route operation: " + operationKey);
}

function resolveObject(value) {
  if (value && typeof value === "object" && typeof value.$ref === "string") {
    return resolveReference(value.$ref);
  }
  return value;
}

const expectedParameters = new Map([
  ["getHealth", []],
  ["listProjects", []],
  ["getProjectStatus", ["path:projectKey"]],
  ["listProjectRefs", ["path:projectKey"]],
  ["listProjectReflog", ["path:projectKey", "query:after_event_id", "query:limit", "query:ref_name"]],
  ["listCreatorSessions", ["path:projectKey"]],
  ["beginCreatorSession", ["path:projectKey"]],
  ["getCreatorSession", ["path:projectKey", "path:session"]],
  ["getCreatorSessionImage", ["path:projectKey", "path:role", "path:session"]],
  ["decideCreatorSession", ["path:projectKey", "path:session"]],
  ["getCreatorSessionDiagnostics", ["path:projectKey", "path:session"]],
  ["startFsck", ["path:projectKey"]],
  ["startArchiveExport", ["path:projectKey"]],
  ["startArchiveRestore", ["path:projectKey"]],
  ["listArchives", []],
  ["getOperation", ["path:operationId"]],
]);

for (const { operation, parameters } of operations) {
  const resolvedParameters = parameters.map((parameter) => resolveObject(parameter));
  const actual = resolvedParameters
    .map((parameter) => parameter?.in + ":" + parameter?.name)
    .sort();
  const expected = expectedParameters.get(operation.operationId) ?? [];
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    fail(operation.operationId + " has unexpected or missing parameters: " + actual.join(", "));
  }
  for (const parameter of resolvedParameters) {
    if (
      !parameter ||
      typeof parameter.name !== "string" ||
      !["path", "query", "header", "cookie"].includes(parameter.in) ||
      !parameter.schema
    ) {
      fail(operation.operationId + " has a malformed parameter");
    }
    if (parameter?.in === "path" && parameter.required !== true) {
      fail(operation.operationId + " has a non-required path parameter " + parameter.name);
    }
  }

  for (const [status, response] of Object.entries(operation.responses ?? {})) {
    const resolvedResponse = resolveObject(response);
    if (typeof resolvedResponse?.description !== "string" || resolvedResponse.description.length === 0) {
      fail(operation.operationId + " has a malformed " + status + " response");
    }
  }
}

const forbiddenWriteFields = new Set([
  "repository_path",
  "archive_path",
  "ref_name",
  "expected_head",
  "new_head",
  "actor_oid",
  "policy_oid",
  "grant_oid",
  "context_pack_oid",
  "authority",
  "authority_profile",
  "registration",
  "permit",
  "head",
  "oid",
  "capabilities",
  "credential",
  "clock",
]);

function collectSchemaProperties(schema, properties, visitedReferences = new Set()) {
  if (schema === null || typeof schema !== "object") {
    return;
  }
  if (typeof schema.$ref === "string") {
    if (visitedReferences.has(schema.$ref)) {
      return;
    }
    visitedReferences.add(schema.$ref);
    collectSchemaProperties(resolveReference(schema.$ref), properties, visitedReferences);
  }
  for (const key of Object.keys(schema.properties ?? {})) {
    properties.add(key);
    collectSchemaProperties(schema.properties[key], properties, visitedReferences);
  }
  for (const key of ["allOf", "anyOf", "oneOf", "prefixItems"]) {
    for (const child of schema[key] ?? []) {
      collectSchemaProperties(child, properties, visitedReferences);
    }
  }
  collectSchemaProperties(schema.items, properties, visitedReferences);
}

const expectedWrites = new Map([
  [
    "beginCreatorSession",
    {
      mediaType: "multipart/form-data",
      properties: ["ai_output", "creator_name", "current_image", "original_image", "session", "subject_label"],
      required: ["ai_output", "creator_name", "current_image", "original_image", "session", "subject_label"],
    },
  ],
  [
    "decideCreatorSession",
    {
      mediaType: "application/json",
      properties: ["disposition", "rationale", "review_id"],
      required: ["disposition", "review_id"],
    },
  ],
  [
    "startFsck",
    {
      mediaType: "application/json",
      properties: ["confirm_project_key"],
      required: ["confirm_project_key"],
    },
  ],
  [
    "startArchiveExport",
    {
      mediaType: "application/json",
      properties: ["archive_name", "confirm_project_key"],
      required: ["archive_name", "confirm_project_key"],
    },
  ],
  [
    "startArchiveRestore",
    {
      mediaType: "application/json",
      properties: ["archive_name", "confirm_empty_target", "confirm_target_project_key"],
      required: ["archive_name", "confirm_empty_target", "confirm_target_project_key"],
    },
  ],
]);

for (const { route, method, operation } of operations.filter(({ method }) => method === "post")) {
  const expectedWrite = expectedWrites.get(operation.operationId);
  const mediaTypes = Object.keys(operation.requestBody?.content ?? {});
  if (
    !expectedWrite ||
    mediaTypes.length !== 1 ||
    mediaTypes[0] !== expectedWrite.mediaType
  ) {
    fail("POST " + route + " has an unexpected request media type");
    continue;
  }
  const rootSchema = resolveObject(operation.requestBody.content[mediaTypes[0]].schema);
  if (!rootSchema || rootSchema.additionalProperties !== false) {
    fail("POST " + route + " must use a closed root request schema");
    continue;
  }
  const rootProperties = Object.keys(rootSchema.properties ?? {}).sort();
  const rootRequired = [...(rootSchema.required ?? [])].sort();
  if (JSON.stringify(rootProperties) !== JSON.stringify(expectedWrite.properties)) {
    fail("POST " + route + " request properties differ from the frozen allowlist");
  }
  if (JSON.stringify(rootRequired) !== JSON.stringify(expectedWrite.required)) {
    fail("POST " + route + " required properties differ from the frozen allowlist");
  }

  const properties = new Set();
  for (const mediaType of Object.values(operation.requestBody?.content ?? {})) {
    collectSchemaProperties(mediaType.schema, properties);
  }
  for (const field of properties) {
    const normalized = field
      .replaceAll(/([a-z0-9])([A-Z])/g, "$1_$2")
      .replaceAll("-", "_")
      .toLowerCase();
    const controlTokens = new Set(normalized.split("_"));
    if (
      forbiddenWriteFields.has(normalized) ||
      ["authority", "capability", "capabilities", "clock", "credential", "head", "oid", "path", "permit"].some(
        (token) => controlTokens.has(token),
      )
    ) {
      fail("POST " + route + " exposes forbidden write field " + field);
    }
  }
}

function requireUtf8ByteLimit(schema, expectedBytes, location) {
  if (schema?.["x-synapse-max-utf8-bytes"] !== expectedBytes) {
    fail(location + " must declare its exact UTF-8 byte limit");
  }
}

const beginSchema = contract.components?.schemas?.BeginCreatorSessionRequest;
const beginOperation = operations.find(
  ({ operation }) => operation.operationId === "beginCreatorSession",
)?.operation;
const beginEncoding = beginOperation?.requestBody?.content?.["multipart/form-data"]?.encoding;
if (
  beginSchema?.["x-synapse-max-file-parts"] !== 3 ||
  beginSchema?.["x-synapse-max-aggregate-file-bytes"] !== 201326592
) {
  fail("creator multipart request must freeze its field-count and aggregate byte limits");
}
requireUtf8ByteLimit(beginSchema?.properties?.subject_label, 500, "subject_label");
requireUtf8ByteLimit(beginSchema?.properties?.creator_name, 300, "creator_name");
for (const field of ["original_image", "current_image", "ai_output"]) {
  const schema = beginSchema?.properties?.[field];
  // OAS 3.1 raw binary omits JSON Schema type/contentEncoding; maxLength counts octets.
  if (
    schema?.contentMediaType !== "application/octet-stream" ||
    schema?.maxLength !== 67108864 ||
    schema?.["x-synapse-raw-binary"] !== true ||
    schema?.["x-synapse-max-raw-bytes"] !== 67108864 ||
    Object.hasOwn(schema ?? {}, "type") ||
    Object.hasOwn(schema ?? {}, "contentEncoding") ||
    beginEncoding?.[field]?.contentType !== "application/octet-stream"
  ) {
    fail(field + " must be a bounded raw OpenAPI 3.1 binary field");
  }
}

const imageOperation = operations.find(
  ({ operation }) => operation.operationId === "getCreatorSessionImage",
)?.operation;
if (imageOperation?.["x-synapse-max-response-bytes"] !== 67108864) {
  fail("creator image responses must declare the 64 MiB server-side ceiling");
}
const expectedImageMedia = [
  "application/octet-stream",
  "image/gif",
  "image/jpeg",
  "image/png",
  "image/webp",
];
const imageContent = imageOperation?.responses?.["200"]?.content ?? {};
if (JSON.stringify(Object.keys(imageContent).sort()) !== JSON.stringify(expectedImageMedia)) {
  fail("creator image response media types differ from the frozen allowlist");
}
for (const [mediaType, media] of Object.entries(imageContent)) {
  if (
    media?.schema?.maxLength !== 67108864 ||
    media?.schema?.["x-synapse-max-raw-bytes"] !== 67108864 ||
    Object.hasOwn(media?.schema ?? {}, "type") ||
    Object.hasOwn(media?.schema ?? {}, "contentEncoding")
  ) {
    fail(mediaType + " must be a bounded raw OpenAPI 3.1 image response");
  }
}
for (const field of ["session", "subject_label", "creator_name"]) {
  if (beginEncoding?.[field]?.contentType !== "text/plain; charset=utf-8") {
    fail(field + " must use an explicit UTF-8 multipart text encoding");
  }
}
requireUtf8ByteLimit(
  contract.components?.schemas?.CreatorDecisionRequest?.properties?.rationale,
  5000,
  "decision rationale",
);
requireUtf8ByteLimit(
  contract.components?.schemas?.CreatorReport?.properties?.rationale,
  5000,
  "report rationale",
);

const projectionFingerprint =
  contract.components?.schemas?.SnapshotContext?.properties?.projection_source_fingerprint;
if (projectionFingerprint?.pattern !== "^projection-source-v1:sha256:[0-9a-f]{64}$") {
  fail("projection fingerprint must preserve the implemented projection-source-v1 format");
}
const creatorReport = contract.components?.schemas?.CreatorReport?.properties;
if (
  creatorReport?.proposal_attributed_to_agent?.type !== "string" ||
  creatorReport?.reviewed_by_human?.type !== "string"
) {
  fail("creator attribution fields must remain EntityId strings");
}

for (const [name, schema] of Object.entries(contract.components?.schemas ?? {})) {
  if (Array.isArray(schema.required)) {
    for (const property of schema.required) {
      if (!Object.hasOwn(schema.properties ?? {}, property)) {
        fail("schema " + name + " requires missing property " + property);
      }
    }
  }
}

const requiredForbiddenCapabilities = new Set([
  "update-ref",
  "raw-object-put",
  "arbitrary-head-publication",
  "authority-profile-administration",
  "permit-exposure",
]);
for (const capability of contract["x-synapse-forbidden-route-capabilities"] ?? []) {
  requiredForbiddenCapabilities.delete(capability);
}
for (const capability of requiredForbiddenCapabilities) {
  fail("missing forbidden-route declaration: " + capability);
}

if (failures.length > 0) {
  for (const failure of failures) {
    console.error("local_api_error: " + failure);
  }
  process.exitCode = 1;
} else {
  console.log(
    "local_api_ok: operations=" +
      operations.length +
      " schemas=" +
      Object.keys(contract.components?.schemas ?? {}).length +
      " refs=" +
      referenceCount,
  );
}
