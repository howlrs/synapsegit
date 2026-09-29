#!/usr/bin/env node
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

function kind(value) {
  if (value === null) return "null";
  if (Array.isArray(value)) return "array";
  return typeof value;
}

function fail(path, message) {
  throw new Error(`report mismatch at ${path}: ${message}`);
}

/**
 * Checks that every value published by an older report is still present and
 * unchanged. Current v1 reports may add properties to any object.
 */
export function assertBackwardCompatible(oldValue, currentValue, path = "$") {
  const oldKind = kind(oldValue);
  const currentKind = kind(currentValue);
  if (oldKind !== currentKind) fail(path, `expected ${oldKind}, got ${currentKind}`);

  if (oldKind === "array") {
    if (oldValue.length !== currentValue.length) {
      fail(path, `expected array length ${oldValue.length}, got ${currentValue.length}`);
    }
    oldValue.forEach((value, index) => assertBackwardCompatible(value, currentValue[index], `${path}[${index}]`));
    return;
  }
  if (oldKind === "object") {
    for (const key of Object.keys(oldValue)) {
      if (!Object.prototype.hasOwnProperty.call(currentValue, key)) fail(`${path}.${key}`, "missing field");
      assertBackwardCompatible(oldValue[key], currentValue[key], `${path}.${key}`);
    }
    return;
  }
  if (!Object.is(oldValue, currentValue)) fail(path, `expected ${JSON.stringify(oldValue)}, got ${JSON.stringify(currentValue)}`);
}

export function assertExactJson(left, right, path = "$") {
  const leftKind = kind(left);
  const rightKind = kind(right);
  if (leftKind !== rightKind) fail(path, `expected ${leftKind}, got ${rightKind}`);
  if (leftKind === "array") {
    if (left.length !== right.length) fail(path, `expected array length ${left.length}, got ${right.length}`);
    left.forEach((value, index) => assertExactJson(value, right[index], `${path}[${index}]`));
    return;
  }
  if (leftKind === "object") {
    const leftKeys = Object.keys(left).sort();
    const rightKeys = Object.keys(right).sort();
    if (leftKeys.length !== rightKeys.length || leftKeys.some((key, index) => key !== rightKeys[index])) {
      fail(path, "object fields differ");
    }
    leftKeys.forEach((key) => assertExactJson(left[key], right[key], `${path}.${key}`));
    return;
  }
  if (!Object.is(left, right)) fail(path, `expected ${JSON.stringify(left)}, got ${JSON.stringify(right)}`);
}

function expectFailure(action, description) {
  assert.throws(action, /report mismatch/, description);
}

function selfTest() {
  const oldReport = JSON.parse('{"format":"v1","scope":"private_local","ids":{"ref":"a"},"timeline":["base","decision"],"optional":null,"__proto__":"owned JSON field"}');
  assertBackwardCompatible(oldReport, { ...oldReport, added: { future: true }, ids: { ref: "a", head: "b" } });
  expectFailure(() => assertBackwardCompatible(oldReport, { ...oldReport, ids: { ref: "changed" } }), "changed field is rejected");
  expectFailure(() => assertBackwardCompatible(oldReport, { format: "v1", scope: "private_local", ids: {}, timeline: ["base", "decision"], optional: null }), "missing field is rejected");
  expectFailure(() => assertBackwardCompatible(oldReport, { ...oldReport, timeline: ["base", "decision", "extra"] }), "array additions are rejected");
  expectFailure(() => assertBackwardCompatible(oldReport, { ...oldReport, timeline: ["decision", "base"] }), "array order changes are rejected");
  expectFailure(() => assertBackwardCompatible(oldReport, { ...oldReport, optional: "present" }), "null changes are rejected");
  expectFailure(() => assertBackwardCompatible(oldReport, { ...oldReport, ids: "a" }), "type changes are rejected");
  expectFailure(() => assertBackwardCompatible(oldReport, { format: "v1", scope: "private_local", ids: { ref: "a" }, timeline: ["base", "decision"], optional: null }), "own __proto__ fields are not inherited");
  expectFailure(() => assertExactJson({ a: 1 }, { a: 1, b: 2 }), "exact comparison rejects extra fields");
  console.log("creator report comparator self-test passed");
}

function readJson(file) {
  try {
    return JSON.parse(readFileSync(file, "utf8"));
  } catch (error) {
    throw new Error(`cannot parse JSON ${file}: ${error.message}`);
  }
}

const args = process.argv.slice(2);
try {
  if (args.length === 1 && args[0] === "--self-test") selfTest();
  else if (args.length === 3 && args[0] === "--backward-compatible") assertBackwardCompatible(readJson(args[1]), readJson(args[2]));
  else if (args.length === 3 && args[0] === "--exact") assertExactJson(readJson(args[1]), readJson(args[2]));
  else throw new Error("usage: compare_creator_reports.mjs --self-test | --backward-compatible <old.json> <current.json> | --exact <left.json> <right.json>");
} catch (error) {
  console.error(`archive_compatibility_error: ${error.message}`);
  process.exitCode = 1;
}
