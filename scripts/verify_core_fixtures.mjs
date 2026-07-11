#!/usr/bin/env node

import { createHash } from "node:crypto";
import {
  readFileSync,
  readdirSync
} from "node:fs";
import { dirname, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const specDir = join(root, "spec/core/v0.1");
const schemaDir = join(specDir, "schemas");
const fixtureDir = join(specDir, "fixtures");
const printGolden = process.argv.includes("--print-golden");
const defaultResourceLimits = Object.freeze({
  maxInputBytes: 16 * 1024 * 1024,
  maxNestingDepth: 128,
  maxNodes: 100_000,
  maxContainerItems: 50_000
});

class CoreError extends Error {
  constructor(code, message) {
    super(`${code}: ${message}`);
    this.name = "CoreError";
    this.code = code;
  }
}

function fail(code, message) {
  throw new CoreError(code, message);
}

function assert(condition, message) {
  if (!condition) throw new Error(message);
}

function assertError(code, fn, label) {
  try {
    fn();
  } catch (error) {
    if (error instanceof CoreError && error.code === code) return;
    throw new Error(`${label}: expected ${code}, got ${error}`);
  }
  throw new Error(`${label}: expected ${code}, but operation succeeded`);
}

function assertUnicodeScalar(value, label) {
  for (let index = 0; index < value.length; index += 1) {
    const unit = value.charCodeAt(index);
    if (unit >= 0xd800 && unit <= 0xdbff) {
      const next = value.charCodeAt(index + 1);
      if (!(next >= 0xdc00 && next <= 0xdfff)) {
        fail("lone_surrogate", `${label} contains an unpaired high surrogate`);
      }
      index += 1;
    } else if (unit >= 0xdc00 && unit <= 0xdfff) {
      fail("lone_surrogate", `${label} contains an unpaired low surrogate`);
    }
  }
}

function parseStrictJsonBytes(bytes, label = "JSON input", resourceLimits = defaultResourceLimits) {
  const limits = { ...defaultResourceLimits, ...resourceLimits };
  if (bytes.length > limits.maxInputBytes) {
    fail("resource_limit", `${label} exceeds the structured input byte limit`);
  }
  if (limits.maxNestingDepth > 256) {
    fail("resource_limit", `${label} nesting limit exceeds the hard ceiling`);
  }
  if (bytes.length >= 3 && bytes[0] === 0xef && bytes[1] === 0xbb && bytes[2] === 0xbf) {
    fail("bom_forbidden", `${label} starts with a UTF-8 BOM`);
  }
  let text;
  try {
    text = new TextDecoder("utf-8", { fatal: true }).decode(bytes);
  } catch {
    fail("invalid_utf8", `${label} is not valid UTF-8`);
  }
  if (text.charCodeAt(0) === 0xfeff) {
    fail("bom_forbidden", `${label} starts with a BOM`);
  }

  let position = 0;
  let nodeCount = 0;
  const isWhitespace = (char) => char === " " || char === "\t" || char === "\r" || char === "\n";
  const skipWhitespace = () => {
    while (position < text.length && isWhitespace(text[position])) position += 1;
  };
  const syntax = (message) => fail("schema_invalid", `${label} at ${position}: ${message}`);

  function parseString() {
    const start = position;
    if (text[position] !== "\"") syntax("expected string");
    position += 1;
    let closed = false;
    while (position < text.length) {
      const unit = text.charCodeAt(position);
      if (unit === 0x22) {
        position += 1;
        closed = true;
        break;
      }
      if (unit < 0x20) syntax("unescaped control character in string");
      if (unit === 0x5c) {
        position += 1;
        if (position >= text.length) syntax("unterminated escape");
        const escaped = text[position];
        if (escaped === "u") {
          const hex = text.slice(position + 1, position + 5);
          if (!/^[0-9a-fA-F]{4}$/.test(hex)) syntax("invalid Unicode escape");
          position += 5;
          continue;
        }
        if (!/^["\\/bfnrt]$/.test(escaped)) syntax("invalid JSON escape");
      }
      position += 1;
    }
    if (!closed) syntax("unterminated string");
    let value;
    try {
      value = JSON.parse(text.slice(start, position));
    } catch {
      syntax("invalid string token");
    }
    assertUnicodeScalar(value, label);
    return value;
  }

  function parseNumber() {
    const start = position;
    while (
      position < text.length &&
      !isWhitespace(text[position]) &&
      ![",", "]", "}"].includes(text[position])
    ) {
      position += 1;
    }
    const token = text.slice(start, position);
    if (!/^-?(0|[1-9][0-9]*)$/.test(token) || token === "-0") {
      fail("number_token_forbidden", `${label} contains forbidden number token ${token}`);
    }
    const value = Number(token);
    if (!Number.isSafeInteger(value)) {
      fail("unsafe_integer", `${label} contains unsafe integer ${token}`);
    }
    return value;
  }

  function childDepth(depth) {
    const next = depth + 1;
    if (next > limits.maxNestingDepth) {
      fail("resource_limit", `${label} exceeds the structured nesting depth limit`);
    }
    return next;
  }

  function consumeNode() {
    nodeCount += 1;
    if (nodeCount > limits.maxNodes) {
      fail("resource_limit", `${label} exceeds the structured node limit`);
    }
  }

  function parseArray(depth) {
    position += 1;
    skipWhitespace();
    const result = [];
    if (text[position] === "]") {
      position += 1;
      return result;
    }
    while (position < text.length) {
      if (result.length >= limits.maxContainerItems) {
        fail("resource_limit", `${label} exceeds the container item limit`);
      }
      result.push(parseValue(depth));
      skipWhitespace();
      if (text[position] === "]") {
        position += 1;
        return result;
      }
      if (text[position] !== ",") syntax("expected ',' or ']'");
      position += 1;
      skipWhitespace();
    }
    syntax("unterminated array");
  }

  function parseObject(depth) {
    position += 1;
    skipWhitespace();
    const result = Object.create(null);
    const keys = new Set();
    if (text[position] === "}") {
      position += 1;
      return result;
    }
    while (position < text.length) {
      if (keys.size >= limits.maxContainerItems) {
        fail("resource_limit", `${label} exceeds the container item limit`);
      }
      if (text[position] !== "\"") syntax("object key must be a string");
      const key = parseString();
      if (keys.has(key)) fail("duplicate_key", `${label} repeats key ${JSON.stringify(key)}`);
      keys.add(key);
      if (key.normalize("NFC") !== key) {
        fail("key_not_nfc", `${label} key ${JSON.stringify(key)} is not NFC`);
      }
      skipWhitespace();
      if (text[position] !== ":") syntax("expected ':'");
      position += 1;
      skipWhitespace();
      result[key] = parseValue(depth);
      skipWhitespace();
      if (text[position] === "}") {
        position += 1;
        return result;
      }
      if (text[position] !== ",") syntax("expected ',' or '}'");
      position += 1;
      skipWhitespace();
    }
    syntax("unterminated object");
  }

  function parseKeyword(keyword, value) {
    if (text.slice(position, position + keyword.length) !== keyword) {
      syntax(`invalid token, expected ${keyword}`);
    }
    position += keyword.length;
    return value;
  }

  function parseValue(depth) {
    skipWhitespace();
    consumeNode();
    const char = text[position];
    if (char === "{") return parseObject(childDepth(depth));
    if (char === "[") return parseArray(childDepth(depth));
    if (char === "\"") return parseString();
    if (char === "t") return parseKeyword("true", true);
    if (char === "f") return parseKeyword("false", false);
    if (char === "n") return parseKeyword("null", null);
    if (char === "-" || (char >= "0" && char <= "9")) return parseNumber();
    syntax("expected JSON value");
  }

  skipWhitespace();
  const value = parseValue(0);
  skipWhitespace();
  if (position !== text.length) syntax("trailing content");
  return value;
}

function utf16Compare(left, right) {
  const length = Math.min(left.length, right.length);
  for (let index = 0; index < length; index += 1) {
    const difference = left.charCodeAt(index) - right.charCodeAt(index);
    if (difference !== 0) return difference;
  }
  return left.length - right.length;
}

function canonicalJson(value) {
  if (value === null) return "null";
  if (value === true) return "true";
  if (value === false) return "false";
  if (typeof value === "string") {
    assertUnicodeScalar(value, "canonical string");
    return JSON.stringify(value);
  }
  if (typeof value === "number") {
    if (!Number.isSafeInteger(value) || Object.is(value, -0)) {
      fail("number_token_forbidden", "canonical value is not a safe non-negative-zero integer token");
    }
    return String(value);
  }
  if (Array.isArray(value)) {
    return `[${value.map((item) => canonicalJson(item)).join(",")}]`;
  }
  if (typeof value === "object") {
    const keys = Object.keys(value).sort(utf16Compare);
    return `{${keys
      .map((key) => {
        assertUnicodeScalar(key, "canonical key");
        if (key.normalize("NFC") !== key) fail("key_not_nfc", `key ${JSON.stringify(key)} is not NFC`);
        return `${JSON.stringify(key)}:${canonicalJson(value[key])}`;
      })
      .join(",")}}`;
  }
  fail("schema_invalid", `unsupported canonical value type ${typeof value}`);
}

function canonicalBytes(value) {
  return Buffer.from(canonicalJson(value), "utf8");
}

function sha256Hex(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

function structuredOid(value) {
  const prefixByType = { record: "record", tree: "tree", commit: "commit" };
  const prefix = prefixByType[value.object_type];
  if (!prefix) fail("schema_invalid", `unsupported object_type ${value.object_type}`);
  const body = canonicalBytes(value);
  const domain = Buffer.from(`synapsegit/core/v0.1\0${value.object_type}\0${body.length}\0`, "utf8");
  const digest = createHash("sha256").update(domain).update(body).digest("hex");
  return `${prefix}:sg-oid-v1:sha256:${digest}`;
}

function blobOid(bytes) {
  return `blob:sg-oid-v1:sha256:${sha256Hex(bytes)}`;
}

const schemas = new Map();
for (const filename of readdirSync(schemaDir).filter((name) => name.endsWith(".json")).sort()) {
  schemas.set(filename, parseStrictJsonBytes(readFileSync(join(schemaDir, filename)), filename));
}

function decodePointerPart(part) {
  return part.replaceAll("~1", "/").replaceAll("~0", "~");
}

function resolveSchemaRef(reference, baseFile) {
  const [filePart, fragment = ""] = reference.split("#", 2);
  const filename = filePart || baseFile;
  let node = schemas.get(filename);
  if (!node) fail("schema_invalid", `${baseFile} references missing schema ${filename}`);
  if (fragment) {
    if (!fragment.startsWith("/")) fail("schema_invalid", `unsupported schema fragment #${fragment}`);
    for (const part of fragment.slice(1).split("/").map(decodePointerPart)) {
      if (!node || typeof node !== "object" || !(part in node)) {
        fail("schema_invalid", `${reference} does not resolve`);
      }
      node = node[part];
    }
  }
  return { schema: node, file: filename };
}

function walkSchema(value, file, path = "") {
  if (!value || typeof value !== "object") return;
  if (typeof value.$ref === "string") resolveSchemaRef(value.$ref, file);
  if (value.type === "array") {
    if (!["set", "sequence"].includes(value["x-synapse-order"])) {
      fail("schema_invalid", `${file}${path} array lacks x-synapse-order`);
    }
    if (value["x-synapse-order"] === "set" && value.uniqueItems !== true) {
      fail("schema_invalid", `${file}${path} set lacks uniqueItems=true`);
    }
  }
  for (const [key, child] of Object.entries(value)) {
    walkSchema(child, file, `${path}/${key}`);
  }
}

for (const [file, schema] of schemas) walkSchema(schema, file);

const recordTypes = new Set(schemas.get("record-envelope.schema.json").properties.record_type.enum);
const dispatchedTypes = new Set(
  schemas.get("record.schema.json").oneOf.map(({ $ref }) => $ref.replace(".schema.json", "").replaceAll("-", "_"))
);
assert(
  JSON.stringify([...recordTypes].sort()) === JSON.stringify([...dispatchedTypes].sort()),
  "record.schema.json must dispatch every and only RecordEnvelope record_type"
);

function expandSchemaEntries(entries) {
  const result = [];
  const seen = new WeakSet();
  function visit(entry) {
    if (!entry?.schema || typeof entry.schema !== "object" || seen.has(entry.schema)) return;
    seen.add(entry.schema);
    result.push(entry);
    if (typeof entry.schema.$ref === "string") visit(resolveSchemaRef(entry.schema.$ref, entry.file));
    for (const keyword of ["allOf", "oneOf", "anyOf"]) {
      for (const child of entry.schema[keyword] || []) visit({ schema: child, file: entry.file });
    }
  }
  for (const entry of entries) visit(entry);
  return result;
}

function propertySchemas(entries, property) {
  const result = [];
  for (const entry of expandSchemaEntries(entries)) {
    if (entry.schema.properties?.[property]) {
      result.push({ schema: entry.schema.properties[property], file: entry.file });
    } else if (entry.schema.additionalProperties && typeof entry.schema.additionalProperties === "object") {
      result.push({ schema: entry.schema.additionalProperties, file: entry.file });
    }
  }
  return result;
}

function itemSchemas(entries) {
  const result = [];
  for (const entry of expandSchemaEntries(entries)) {
    if (entry.schema.items && typeof entry.schema.items === "object") {
      result.push({ schema: entry.schema.items, file: entry.file });
    }
  }
  return result;
}

const timestampPattern = /^[0-9]{4}-(0[1-9]|1[0-2])-([0-2][0-9]|3[01])T([01][0-9]|2[0-3]):[0-5][0-9]:[0-5][0-9]\.[0-9]{9}Z$/;
const units = new Set(schemas.get("common.schema.json").$defs.Unit.enum);

function validateTimestamp(value) {
  if (!timestampPattern.test(value)) fail("timestamp_invalid", `invalid canonical timestamp ${value}`);
  const year = Number(value.slice(0, 4));
  const month = Number(value.slice(5, 7));
  const day = Number(value.slice(8, 10));
  const leap = year % 4 === 0 && (year % 100 !== 0 || year % 400 === 0);
  const days = [31, leap ? 29 : 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
  if (day > days[month - 1]) fail("timestamp_invalid", `calendar date does not exist: ${value}`);
}

function scaledIntegerToBigRational(value) {
  const mantissa = BigInt(value.mantissa);
  if (value.scale >= 0) return { numerator: mantissa * 10n ** BigInt(value.scale), denominator: 1n };
  return { numerator: mantissa, denominator: 10n ** BigInt(-value.scale) };
}

function compareScaled(left, right) {
  const a = scaledIntegerToBigRational(left);
  const b = scaledIntegerToBigRational(right);
  const difference = a.numerator * b.denominator - b.numerator * a.denominator;
  return difference < 0n ? -1 : difference > 0n ? 1 : 0;
}

function validateScaledInteger(value) {
  if (
    typeof value.mantissa !== "string" ||
    !/^(0|-?(?:[1-9]|[1-9][0-9]*[1-9]))$/.test(value.mantissa) ||
    value.mantissa.length > 257 ||
    !Number.isInteger(value.scale) ||
    value.scale < -24 ||
    value.scale > 24 ||
    !units.has(value.unit) ||
    (value.mantissa === "0" && value.scale !== 0)
  ) {
    fail("fixed_point_not_normalized", `invalid ScaledInteger ${canonicalJson(value)}`);
  }
}

function bufferCompare(left, right) {
  return Buffer.compare(left, right);
}

function validateAnnotated(value, entries, pointer = "") {
  const expanded = expandSchemaEntries(entries);
  if (typeof value === "string") {
    const markers = new Set(expanded.map((entry) => entry.schema["x-synapse-string"]).filter(Boolean));
    if (markers.has("identifier-nfc") && value.normalize("NFC") !== value) {
      fail("identifier_not_nfc", `${pointer || "/"} is not NFC`);
    }
    if (markers.has("canonical-timestamp")) validateTimestamp(value);
    return;
  }
  if (Array.isArray(value)) {
    const orders = new Set(expanded.map((entry) => entry.schema["x-synapse-order"]).filter(Boolean));
    if (orders.size > 1) fail("schema_invalid", `${pointer || "/"} has conflicting array order annotations`);
    const order = [...orders][0] || "sequence";
    if (order === "set") {
      const encoded = value.map((item) => canonicalBytes(item));
      for (let index = 1; index < encoded.length; index += 1) {
        const comparison = bufferCompare(encoded[index - 1], encoded[index]);
        if (comparison === 0) fail("set_duplicate", `${pointer || "/"} has duplicate set items`);
        if (comparison > 0) fail("set_not_sorted", `${pointer || "/"} is not in canonical set order`);
      }
    }
    const children = itemSchemas(expanded);
    value.forEach((child, index) => validateAnnotated(child, children, `${pointer}/${index}`));
    return;
  }
  if (!value || typeof value !== "object") return;

  if (["mantissa", "scale", "unit"].every((key) => Object.hasOwn(value, key))) {
    validateScaledInteger(value);
  }
  if (value.kind === "interval" && value.from && value.to && value.from > value.to) {
    fail("interval_invalid", `${pointer || "/"} has from later than to`);
  }
  if (["resolution", "uncertainty"].includes(value.kind) && value.value?.mantissa !== undefined) {
    validateScaledInteger(value.value);
    if (value.value.mantissa.startsWith("-") || !["ms", "s"].includes(value.value.unit)) {
      fail("fixed_point_not_normalized", `${pointer || "/"} has invalid temporal precision`);
    }
  }

  for (const [key, child] of Object.entries(value)) {
    if (pointer.endsWith("/entries")) {
      if (!key || key === "." || key === ".." || key.includes("/") || key.includes("\0")) {
        fail("path_segment_invalid", `invalid Manifest segment ${JSON.stringify(key)}`);
      }
    }
    validateAnnotated(child, propertySchemas(expanded, key), `${pointer}/${key.replaceAll("~", "~0").replaceAll("/", "~1")}`);
  }
}

function schemaFileForObject(value) {
  if (value.object_type === "tree") return "manifest-tree.schema.json";
  if (value.object_type === "commit") return "commit.schema.json";
  if (value.object_type === "record") return `${value.record_type.replaceAll("_", "-")}.schema.json`;
  fail("schema_invalid", `unknown object_type ${value.object_type}`);
}

function validateObject(value) {
  const schemaFile = schemaFileForObject(value);
  const schema = schemas.get(schemaFile);
  if (!schema) fail("schema_invalid", `missing concrete schema ${schemaFile}`);
  validateAnnotated(value, [{ schema, file: schemaFile }]);

  if (value.object_type === "tree") {
    for (const [segment, entry] of Object.entries(value.entries)) {
      const prefix = entry.oid?.split(":", 1)[0];
      if (prefix !== entry.entry_kind) {
        fail("reference_type_mismatch", `${segment} says ${entry.entry_kind} but uses ${entry.oid}`);
      }
    }
  }
  if (value.record_type === "observation" && value.valid_time !== undefined) {
    fail("schema_invalid", "Observation v0.1 must use payload.capture_time, not Envelope valid_time");
  }
  if (value.record_type === "delegation_grant") {
    validateTimestamp(value.payload.expires_at);
    if (value.payload.expires_at < value.recorded_at) {
      fail("interval_invalid", "DelegationGrant expires before recorded_at");
    }
  }
}

const fixtureObjects = new Map();
const objectRows = [];
for (const filename of readdirSync(fixtureDir).filter((name) => name.endsWith(".json") && name !== "golden.json").sort()) {
  const bytes = readFileSync(join(fixtureDir, filename));
  const value = parseStrictJsonBytes(bytes, filename);
  validateObject(value);
  assert(!Object.hasOwn(value, "oid"), `${filename} must not embed its own OID`);
  const canonical = canonicalBytes(value);
  const oid = structuredOid(value);
  fixtureObjects.set(filename, { value, oid, canonical });
  objectRows.push({
    path: `fixtures/${filename}`,
    canonical_length: canonical.length,
    canonical_sha256: sha256Hex(canonical),
    oid
  });
}

const blobBytes = readFileSync(join(fixtureDir, "proposal.txt"));
const proposalBlobOid = blobOid(blobBytes);
const generatedGolden = {
  profile: "synapsegit/core/v0.1",
  objects: objectRows,
  blobs: [
    {
      path: "fixtures/proposal.txt",
      byte_length: blobBytes.length,
      sha256: sha256Hex(blobBytes),
      oid: proposalBlobOid
    }
  ],
  equivalent_groups: [
    ["fixtures/actor-creator-a.json", "fixtures/actor-creator-b.json"],
    ["fixtures/base-tree-a.json", "fixtures/base-tree-b.json"]
  ],
  distinct_pairs: [
    ["fixtures/actor-creator-a.json", "fixtures/actor-creator-nfd.json"],
    ["fixtures/merge-commit.json", "fixtures/merge-commit-swapped.json"]
  ]
};

if (printGolden) {
  process.stdout.write(`${JSON.stringify(generatedGolden, null, 2)}\n`);
  process.exit(0);
}

const goldenPath = join(fixtureDir, "golden.json");
const golden = parseStrictJsonBytes(readFileSync(goldenPath), "golden.json");
assert(
  canonicalJson(golden) === canonicalJson(generatedGolden),
  "computed canonical lengths, hashes, or OIDs differ from fixtures/golden.json"
);

function byPath(path) {
  return fixtureObjects.get(path.replace("fixtures/", ""));
}

for (const group of golden.equivalent_groups) {
  const [first, ...rest] = group.map(byPath);
  for (const item of rest) {
    assert(first.oid === item.oid, `${group.join(", ")} must have equal OIDs`);
    assert(first.canonical.equals(item.canonical), `${group.join(", ")} must have equal canonical bytes`);
  }
}
for (const [leftPath, rightPath] of golden.distinct_pairs) {
  assert(byPath(leftPath).oid !== byPath(rightPath).oid, `${leftPath} and ${rightPath} must have distinct OIDs`);
}

const orderedTree = canonicalJson(byPath("fixtures/base-tree-a.json").value);
const orderPositions = [
  orderedTree.indexOf('"10"'),
  orderedTree.indexOf('"2"'),
  orderedTree.indexOf(JSON.stringify("\u{10000}")),
  orderedTree.indexOf(JSON.stringify("\ue000"))
];
assert(orderPositions.every((position) => position >= 0), "ordering sentinel keys must exist");
assert(
  orderPositions.every((position, index) => index === 0 || orderPositions[index - 1] < position),
  "object keys must be emitted in UTF-16 code-unit order"
);

assertError("invalid_utf8", () => parseStrictJsonBytes(Buffer.from([0x22, 0xc3, 0x28, 0x22])), "invalid UTF-8");
assertError(
  "bom_forbidden",
  () => parseStrictJsonBytes(Buffer.concat([Buffer.from([0xef, 0xbb, 0xbf]), Buffer.from("{}")])) ,
  "input BOM"
);
assertError("duplicate_key", () => parseStrictJsonBytes(Buffer.from('{"a":1,"\\u0061":2}')), "decoded duplicate key");
assertError("number_token_forbidden", () => parseStrictJsonBytes(Buffer.from('{"a":1.0}')), "fraction token");
assertError("number_token_forbidden", () => parseStrictJsonBytes(Buffer.from('{"a":1e0}')), "exponent token");
assertError("number_token_forbidden", () => parseStrictJsonBytes(Buffer.from('{"a":-0}')), "negative zero token");
assertError("unsafe_integer", () => parseStrictJsonBytes(Buffer.from('{"a":9007199254740992}')), "unsafe integer");
assertError("lone_surrogate", () => parseStrictJsonBytes(Buffer.from('{"a":"\\ud800"}')), "lone surrogate");
assertError("key_not_nfc", () => parseStrictJsonBytes(Buffer.from('{"e\\u0301":1}')), "non-NFC key");

const oneLevelLimits = { ...defaultResourceLimits, maxNestingDepth: 1 };
parseStrictJsonBytes(Buffer.from("[0]"), "one-level JSON", oneLevelLimits);
assertError(
  "resource_limit",
  () => parseStrictJsonBytes(Buffer.from("[[0]]"), "nested JSON", oneLevelLimits),
  "nesting resource limit"
);
const twoNodeLimits = { ...defaultResourceLimits, maxNodes: 2 };
assertError(
  "resource_limit",
  () => parseStrictJsonBytes(Buffer.from("[0,1]"), "three-node JSON", twoNodeLimits),
  "node resource limit"
);
const oneItemLimits = { ...defaultResourceLimits, maxContainerItems: 1 };
assertError(
  "resource_limit",
  () => parseStrictJsonBytes(Buffer.from("[0,1]"), "two-item JSON", oneItemLimits),
  "container resource limit"
);
assertError(
  "resource_limit",
  () => parseStrictJsonBytes(Buffer.from("null"), "unsafe limit", {
    ...defaultResourceLimits,
    maxNestingDepth: 257
  }),
  "nesting hard ceiling"
);

const badIdentifier = structuredClone(byPath("fixtures/context-pack.json").value);
badIdentifier.payload.base_ref_name = "decision/cafe\u0301";
assertError("identifier_not_nfc", () => validateObject(badIdentifier), "non-NFC identifier");

const badPath = structuredClone(byPath("fixtures/base-tree-a.json").value);
badPath.entries["bad/segment"] = badPath.entries["10"];
assertError("path_segment_invalid", () => validateObject(badPath), "Manifest separator");

const badTimestamp = structuredClone(byPath("fixtures/actor-creator-a.json").value);
badTimestamp.recorded_at = "2026-02-30T00:00:00.000000000Z";
assertError("timestamp_invalid", () => validateObject(badTimestamp), "nonexistent date");
const shortTimestamp = structuredClone(byPath("fixtures/actor-creator-a.json").value);
shortTimestamp.recorded_at = "2026-07-11T00:00:00Z";
assertError("timestamp_invalid", () => validateObject(shortTimestamp), "timestamp precision");
const offsetTimestamp = structuredClone(byPath("fixtures/actor-creator-a.json").value);
offsetTimestamp.recorded_at = "2026-07-11T00:00:00.000000000+00:00";
assertError("timestamp_invalid", () => validateObject(offsetTimestamp), "timestamp offset");

const unsortedSet = structuredClone(byPath("fixtures/actor-ai.json").value);
unsortedSet.payload.ai_profile.capabilities = ["read_context", "analyze"];
assertError("set_not_sorted", () => validateObject(unsortedSet), "unsorted set");
const duplicateSet = structuredClone(byPath("fixtures/actor-ai.json").value);
duplicateSet.payload.ai_profile.capabilities = ["analyze", "analyze"];
assertError("set_duplicate", () => validateObject(duplicateSet), "duplicate set");

const badFixedPoint = structuredClone(byPath("fixtures/delegation-grant.json").value);
badFixedPoint.valid_time.precision.value.mantissa = "10";
assertError("fixed_point_not_normalized", () => validateObject(badFixedPoint), "trailing-zero mantissa");
const badZeroScale = structuredClone(byPath("fixtures/delegation-grant.json").value);
badZeroScale.valid_time.precision.value = { mantissa: "0", scale: -1, unit: "s" };
assertError("fixed_point_not_normalized", () => validateObject(badZeroScale), "noncanonical zero scale");
const badUnit = structuredClone(byPath("fixtures/delegation-grant.json").value);
badUnit.valid_time.precision.value.unit = "frame";
assertError("fixed_point_not_normalized", () => validateObject(badUnit), "unknown unit");

const invertedInterval = structuredClone(byPath("fixtures/delegation-grant.json").value);
invertedInterval.valid_time = {
  kind: "interval",
  from: "2026-07-12T00:00:00.000000000Z",
  to: "2026-07-11T00:00:00.000000000Z"
};
assertError("interval_invalid", () => validateObject(invertedInterval), "inverted interval");

const wrongEntryKind = structuredClone(byPath("fixtures/base-tree-a.json").value);
wrongEntryKind.entries["10"].entry_kind = "blob";
assertError("reference_type_mismatch", () => validateObject(wrongEntryKind), "Manifest kind/OID mismatch");

function verifyClaimedOid(claimedOid, body) {
  const expected = structuredOid(body);
  if (claimedOid !== expected) fail("oid_mismatch", `claimed ${claimedOid}, expected ${expected}`);
}
assertError(
  "oid_mismatch",
  () => verifyClaimedOid(byPath("fixtures/actor-creator-a.json").oid, byPath("fixtures/base-tree-a.json").value),
  "OID wrapper/body mismatch"
);
assert(
  blobOid(byPath("fixtures/actor-creator-a.json").canonical) !== byPath("fixtures/actor-creator-a.json").oid,
  "Blob and structured object domains must not share an OID"
);

function collectOids(value, result = new Set()) {
  if (typeof value === "string" && /^(blob|record|tree|commit):sg-oid-v1:sha256:[0-9a-f]{64}$/.test(value)) {
    result.add(value);
  } else if (Array.isArray(value)) {
    for (const child of value) collectOids(child, result);
  } else if (value && typeof value === "object") {
    for (const child of Object.values(value)) collectOids(child, result);
  }
  return result;
}

const store = new Map();
for (const { value, oid } of fixtureObjects.values()) store.set(oid, value);
store.set(proposalBlobOid, blobBytes);
const tombstones = new Map();
for (const { value, oid } of fixtureObjects.values()) {
  if (value.record_type === "tombstone") tombstones.set(value.payload.target_ref, oid);
}

function resolveAvailability(oid, objectStore) {
  if (objectStore.has(oid)) return "present";
  if (tombstones.has(oid)) return "tombstoned";
  return "missing";
}

function verifyClosure(head, objectStore) {
  const queue = [head];
  const visited = new Set();
  const states = new Map();
  while (queue.length) {
    const oid = queue.pop();
    if (visited.has(oid)) continue;
    visited.add(oid);
    const state = resolveAvailability(oid, objectStore);
    states.set(oid, state);
    if (state === "missing") fail("closure_missing", `missing ${oid}`);
    if (state === "tombstoned" || oid.startsWith("blob:")) continue;
    const body = objectStore.get(oid);
    assert(structuredOid(body) === oid, `stored object ${oid} must verify`);
    for (const child of collectOids(body)) queue.push(child);
  }
  return states;
}

const proposalHead = byPath("fixtures/proposal-commit.json").oid;
const allPresent = verifyClosure(proposalHead, store);
assert([...allPresent.values()].every((state) => state === "present"), "full fixture closure must be present");

const erasedStore = new Map(store);
erasedStore.delete(proposalBlobOid);
const erasedClosure = verifyClosure(proposalHead, erasedStore);
assert(erasedClosure.get(proposalBlobOid) === "tombstoned", "erased proposal blob must resolve as tombstoned");

const missingStore = new Map(store);
const policyOid = byPath("fixtures/policy.json").oid;
missingStore.delete(policyOid);
assertError("closure_missing", () => verifyClosure(proposalHead, missingStore), "missing closure object");

const restoredStore = new Map();
for (const [oid, body] of store) {
  if (oid.startsWith("blob:")) restoredStore.set(blobOid(body), Buffer.from(body));
  else {
    const restored = parseStrictJsonBytes(canonicalBytes(body), oid);
    restoredStore.set(structuredOid(restored), restored);
  }
}
assert(restoredStore.size === store.size, "empty-store restore must preserve object count");
verifyClosure(proposalHead, restoredStore);

process.stdout.write(
  [
    `ok: ${schemas.size} schemas parsed and references resolved`,
    `ok: ${objectRows.length} structured fixtures match golden OIDs`,
    "ok: strict JSON, Unicode, number, resource-limit, set, time, fixed-point, and direction cases",
    "ok: present / tombstoned / missing closure and empty-store restore"
  ].join("\n") + "\n"
);
