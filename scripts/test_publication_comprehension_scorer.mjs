#!/usr/bin/env node

import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync, cpSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

import {
  loadPublicationComprehensionCorpus,
  meetsPercentThreshold,
  publicationComprehensionCorpusDir,
  scorePublicationComprehension,
  scorePublicationComprehensionRecords,
} from "./score_publication_comprehension.mjs";

const scriptDir = path.dirname(fileURLToPath(import.meta.url));
const scorerScriptPath = path.join(scriptDir, "score_publication_comprehension.mjs");

const corpus = loadPublicationComprehensionCorpus();

const CORPUS_ERROR_PREFIX = "publication comprehension corpus error:";

/**
 * Copy the frozen v1 corpus into a fresh temp directory so a test can
 * mutate one or more JSON fixtures without touching the repository's
 * frozen area. `mutations` maps a relative JSON path to a mutator function
 * (or an array of relative paths sharing one mutator, when a single
 * scenario must keep two documents mutually consistent, e.g. removing a
 * question from questionnaire.json while also removing its oracle answer).
 */
function withMutatedCorpus(mutations, run) {
  const tempDir = mkdtempSync(path.join(tmpdir(), "publication-comprehension-corpus-"));
  try {
    cpSync(publicationComprehensionCorpusDir, tempDir, { recursive: true });
    for (const [relativeJsonPath, mutate] of Object.entries(mutations)) {
      const targetPath = path.join(tempDir, relativeJsonPath);
      const document = JSON.parse(readFileSync(targetPath, "utf8"));
      mutate(document);
      writeFileSync(targetPath, JSON.stringify(document, null, 2));
    }
    return run(tempDir);
  } finally {
    rmSync(tempDir, { recursive: true, force: true });
  }
}

/**
 * Assert that loading a corpus mutated per `mutations` (see
 * withMutatedCorpus) throws the frozen-corpus error with the expected
 * prefix and a message substring specific enough to prove which validation
 * fired, not merely that some validation fired.
 */
function assertLoadThrowsMulti(mutations, expectedMessageSubstring) {
  assert.throws(
    () => {
      withMutatedCorpus(mutations, (tempDir) => loadPublicationComprehensionCorpus(tempDir));
    },
    (error) => {
      assert.ok(error instanceof Error, "expected an Error to be thrown");
      assert.ok(
        error.message.startsWith(CORPUS_ERROR_PREFIX),
        `expected corpus error prefix, got: ${error.message}`,
      );
      assert.ok(
        error.message.includes(expectedMessageSubstring),
        `expected message to include ${JSON.stringify(expectedMessageSubstring)}, got: ${error.message}`,
      );
      return true;
    },
  );
}

function assertLoadThrows(relativeJsonPath, mutate, expectedMessageSubstring) {
  assertLoadThrowsMulti({ [relativeJsonPath]: mutate }, expectedMessageSubstring);
}

function answersFor(caseId, track) {
  return Object.fromEntries(
    corpus.questionsByCaseTrack
      .get(`${caseId}\u0000${track}`)
      .map((question) => [question.id, question.expected]),
  );
}

function responseFor({
  caseId,
  track,
  evaluatorKind,
  runId,
  answers = answersFor(caseId, track),
}) {
  return {
    schema: {
      name: "org.synapsegit.publication-comprehension-response",
      version: 1,
    },
    corpus_version: corpus.corpusVersion,
    case_id: caseId,
    track,
    evaluator_kind: evaluatorKind,
    run_id: runId,
    input_artifact_sha256: corpus.artifactDigests.get(`${caseId}\u0000${track}`),
    evaluator_metadata:
      evaluatorKind === "human"
        ? { browser_id: "test-browser" }
        : { model_id: "test-model", model_configuration: "temperature=0" },
    answers,
  };
}

function findGroup(result, caseId, track, evaluatorKind) {
  const group = result.groups.find(
    (candidate) =>
      candidate.case_id === caseId &&
      candidate.track === track &&
      candidate.evaluator_kind === evaluatorKind,
  );
  assert.ok(group, `missing group ${caseId}/${track}/${evaluatorKind}`);
  return group;
}

function aiGroupResponses(caseId, track, mutate = () => {}) {
  return Array.from({ length: corpus.thresholds.aiMinimumRuns }, (_, index) => {
    const response = responseFor({
      caseId,
      track,
      evaluatorKind: "zero_context_ai",
      runId: `ai-${caseId}-${track}-${index + 1}`,
    });
    mutate(response, index);
    return response;
  });
}

function humanGroupResponses(caseId, mutate = () => {}) {
  return Array.from({ length: corpus.thresholds.humanMinimumParticipants }, (_, index) => {
    const response = responseFor({
      caseId,
      track: "html",
      evaluatorKind: "human",
      runId: `human-${caseId}-${index + 1}`,
    });
    mutate(response, index);
    return response;
  });
}

function testCorpusAndIntegerThresholds() {
  assert.equal(corpus.questionsByCaseTrack.get("incomplete-only\u0000json").length, 15);
  assert.equal(corpus.questionsByCaseTrack.get("incomplete-only\u0000html").length, 14);
  assert.ok(
    corpus.questionsByCaseTrack
      .get("incomplete-only\u0000json")
      .some((question) => question.id === "I06"),
  );
  assert.ok(
    !corpus.questionsByCaseTrack
      .get("incomplete-only\u0000html")
      .some((question) => question.id === "I06"),
  );
  assert.equal(meetsPercentThreshold(17, 20, 85), true);
  assert.equal(meetsPercentThreshold(16, 19, 85), false);
  assert.equal(meetsPercentThreshold(9, 10, 90), true);
}

function testFrozenCorpusThresholdsRegression() {
  // Frozen v1 corpus regression guard: loading must keep succeeding and the
  // threshold values must not silently drift as new corpus-load validation
  // is layered on top of loadPublicationComprehensionCorpus.
  assert.equal(corpus.corpusVersion, 1);
  assert.deepEqual([...corpus.cases], ["complete", "incomplete-only"]);
  assert.equal(corpus.thresholds.aiMinimumRuns, 3);
  assert.equal(corpus.thresholds.aiJsonMinimumPercent, 95);
  assert.equal(corpus.thresholds.aiHtmlMinimumPercent, 90);
  assert.equal(corpus.thresholds.humanMinimumParticipants, 10);
  assert.equal(corpus.thresholds.humanMinimumMacroPercent, 85);
  assert.equal(corpus.thresholds.humanCriticalMinimumPercent, 90);
  assert.equal(corpus.thresholds.aiAllCriticalRequired, true);
  assert.equal(corpus.thresholds.humanAllCriticalRequired, true);
}

function testCorpusLoadRejectsSchemaIdentityTampering() {
  assertLoadThrows(
    "questionnaire.json",
    (doc) => {
      doc.schema.name = "org.synapsegit.wrong-questionnaire-schema";
    },
    "questionnaire schema",
  );
  assertLoadThrows(
    "questionnaire.json",
    (doc) => {
      doc.schema.version = 2;
    },
    "questionnaire schema",
  );
  assertLoadThrows(
    "questionnaire.json",
    (doc) => {
      doc.schema.extra_property = "unexpected";
    },
    "questionnaire schema",
  );
  assertLoadThrows(
    "oracle.json",
    (doc) => {
      doc.schema.name = "org.synapsegit.wrong-oracle-schema";
    },
    "oracle schema",
  );
  assertLoadThrows(
    "oracle.json",
    (doc) => {
      doc.schema.version = 2;
    },
    "oracle schema",
  );
  assertLoadThrows(
    "oracle.json",
    (doc) => {
      doc.schema.extra_property = "unexpected";
    },
    "oracle schema",
  );
  assertLoadThrows(
    "protocol.json",
    (doc) => {
      doc.schema.name = "org.synapsegit.wrong-protocol-schema";
    },
    "protocol schema",
  );
  assertLoadThrows(
    "protocol.json",
    (doc) => {
      doc.schema.version = 2;
    },
    "protocol schema",
  );
  assertLoadThrows(
    "protocol.json",
    (doc) => {
      doc.schema.extra_property = "unexpected";
    },
    "protocol schema",
  );
}

function testCorpusLoadRejectsTrackMatrixMismatch() {
  assertLoadThrows(
    "protocol.json",
    (doc) => {
      doc.context.track_matrix = [
        { evaluator_kind: "zero_context_ai", track: "json" },
        { evaluator_kind: "zero_context_ai", track: "html" },
      ];
    },
    "track_matrix",
  );
  assertLoadThrows(
    "protocol.json",
    (doc) => {
      // Same members, different order: the matrix is an ordered contract.
      doc.context.track_matrix = [
        { evaluator_kind: "zero_context_ai", track: "html" },
        { evaluator_kind: "zero_context_ai", track: "json" },
        { evaluator_kind: "human", track: "html" },
      ];
    },
    "track_matrix",
  );
  assertLoadThrows(
    "protocol.json",
    (doc) => {
      doc.context.track_matrix.push({ evaluator_kind: "human", track: "json" });
    },
    "track_matrix",
  );
}

function testCorpusLoadRejectsDuplicateCasesAndTracks() {
  assertLoadThrows(
    "questionnaire.json",
    (doc) => {
      const question = doc.questions.find((entry) => entry.id === "P01");
      question.cases = ["complete", "complete", "incomplete-only"];
    },
    "P01",
  );
  assertLoadThrows(
    "questionnaire.json",
    (doc) => {
      const question = doc.questions.find((entry) => entry.id === "P01");
      question.tracks = ["json", "json", "html"];
    },
    "P01",
  );
}

function testCorpusLoadRejectsEmptyCaseTrackApplicability() {
  // Detach every question that applies to both "incomplete-only" and the
  // "json" track from that case/track combination, without ever leaving a
  // question with an empty cases or tracks array:
  //  - questions shared with "complete" (P01-P08) drop the
  //    "incomplete-only" case, keeping ["complete"].
  //  - questions exclusive to "incomplete-only" (I01-I07) drop the "json"
  //    track, keeping ["html"] (I06, which only has ["json"], is removed
  //    outright since dropping its only track would leave an empty tracks
  //    array).
  // oracle.json's incomplete-only case must drop the matching answers in
  // the same mutation so the unrelated "answer count does not match its
  // questionnaire" check does not fire first and mask the validation this
  // test targets.
  const detachedFromIncompleteOnly = new Set();
  assertLoadThrowsMulti(
    {
      "questionnaire.json": (doc) => {
        doc.questions = doc.questions
          .filter((question) => question.id !== "I06")
          .map((question) => {
            if (!question.cases.includes("incomplete-only") || !question.tracks.includes("json")) {
              return question;
            }
            if (question.cases.length > 1) {
              detachedFromIncompleteOnly.add(question.id);
              return {
                ...question,
                cases: question.cases.filter((caseId) => caseId !== "incomplete-only"),
              };
            }
            return { ...question, tracks: question.tracks.filter((track) => track !== "json") };
          });
      },
      "oracle.json": (doc) => {
        // Keep oracle.json's answer set consistent with the mutated
        // questionnaire: drop I06 (removed outright) and every P0x answer
        // detached from "incomplete-only" above, so the pre-existing
        // "answer count does not match its questionnaire" check does not
        // fire first and mask the empty-applicability validation under
        // test.
        delete doc.cases["incomplete-only"].answers.I06;
        for (const questionId of detachedFromIncompleteOnly) {
          delete doc.cases["incomplete-only"].answers[questionId];
        }
      },
    },
    "case incomplete-only track json has no applicable questions",
  );
}

function testCorpusLoadRejectsNonSafeIntegerOracleAnswer() {
  assertLoadThrows(
    "oracle.json",
    (doc) => {
      doc.cases.complete.answers.P01.value = 2 ** 53;
    },
    "P01",
  );
}

function testEvaluatorMetadataValidation() {
  const zeroContext = responseFor({
    caseId: "complete",
    track: "html",
    evaluatorKind: "zero_context_ai",
    runId: "metadata-unknown-property",
  });
  zeroContext.evaluator_metadata.unexpected_field = "value";
  const unknownResult = scorePublicationComprehension([zeroContext], corpus);
  assert.ok(
    unknownResult.responses[0].errors.some(
      (error) => error.code === "unknown_evaluator_metadata_property" && error.property === "unexpected_field",
    ),
  );

  const emptyValue = responseFor({
    caseId: "complete",
    track: "html",
    evaluatorKind: "zero_context_ai",
    runId: "metadata-empty-value",
  });
  emptyValue.evaluator_metadata.model_id = "";
  const emptyResult = scorePublicationComprehension([emptyValue], corpus);
  assert.ok(
    emptyResult.responses[0].errors.some(
      (error) => error.code === "invalid_evaluator_metadata_property" && error.property === "model_id",
    ),
  );

  const tooLong = responseFor({
    caseId: "complete",
    track: "html",
    evaluatorKind: "zero_context_ai",
    runId: "metadata-too-long",
  });
  tooLong.evaluator_metadata.model_id = "x".repeat(257);
  const tooLongResult = scorePublicationComprehension([tooLong], corpus);
  assert.ok(
    tooLongResult.responses[0].errors.some(
      (error) => error.code === "invalid_evaluator_metadata_property" && error.property === "model_id",
    ),
  );

  const nonString = responseFor({
    caseId: "complete",
    track: "html",
    evaluatorKind: "zero_context_ai",
    runId: "metadata-non-string",
  });
  nonString.evaluator_metadata.model_configuration = 42;
  const nonStringResult = scorePublicationComprehension([nonString], corpus);
  assert.ok(
    nonStringResult.responses[0].errors.some(
      (error) =>
        error.code === "invalid_evaluator_metadata_property" && error.property === "model_configuration",
    ),
  );

  const missingModelId = responseFor({
    caseId: "complete",
    track: "html",
    evaluatorKind: "zero_context_ai",
    runId: "metadata-missing-model-id",
  });
  delete missingModelId.evaluator_metadata.model_id;
  const missingModelIdResult = scorePublicationComprehension([missingModelId], corpus);
  assert.ok(
    missingModelIdResult.responses[0].errors.some(
      (error) => error.code === "missing_evaluator_metadata_property" && error.property === "model_id",
    ),
  );

  const missingModelConfiguration = responseFor({
    caseId: "complete",
    track: "html",
    evaluatorKind: "zero_context_ai",
    runId: "metadata-missing-model-configuration",
  });
  delete missingModelConfiguration.evaluator_metadata.model_configuration;
  const missingModelConfigurationResult = scorePublicationComprehension(
    [missingModelConfiguration],
    corpus,
  );
  assert.ok(
    missingModelConfigurationResult.responses[0].errors.some(
      (error) =>
        error.code === "missing_evaluator_metadata_property" && error.property === "model_configuration",
    ),
  );

  const missingBrowserId = responseFor({
    caseId: "complete",
    track: "html",
    evaluatorKind: "human",
    runId: "metadata-missing-browser-id",
  });
  delete missingBrowserId.evaluator_metadata.browser_id;
  const missingBrowserIdResult = scorePublicationComprehension([missingBrowserId], corpus);
  assert.ok(
    missingBrowserIdResult.responses[0].errors.some(
      (error) => error.code === "missing_evaluator_metadata_property" && error.property === "browser_id",
    ),
  );
}

function testRunIdAndNotesBoundaries() {
  const emptyRunId = responseFor({
    caseId: "complete",
    track: "html",
    evaluatorKind: "zero_context_ai",
    runId: "",
  });
  const emptyRunIdResult = scorePublicationComprehension([emptyRunId], corpus);
  assert.ok(
    emptyRunIdResult.responses[0].errors.some((error) => error.code === "invalid_run_id"),
  );

  const tooLongRunId = responseFor({
    caseId: "complete",
    track: "html",
    evaluatorKind: "zero_context_ai",
    runId: "r".repeat(129),
  });
  const tooLongRunIdResult = scorePublicationComprehension([tooLongRunId], corpus);
  assert.ok(
    tooLongRunIdResult.responses[0].errors.some((error) => error.code === "invalid_run_id"),
  );

  const maxRunId = responseFor({
    caseId: "complete",
    track: "html",
    evaluatorKind: "zero_context_ai",
    runId: "r".repeat(128),
  });
  const maxRunIdResult = scorePublicationComprehension([maxRunId], corpus);
  assert.ok(
    !maxRunIdResult.responses[0].errors.some((error) => error.code === "invalid_run_id"),
  );

  const tooLongNotes = responseFor({
    caseId: "complete",
    track: "html",
    evaluatorKind: "zero_context_ai",
    runId: "notes-too-long",
  });
  tooLongNotes.notes = "n".repeat(4001);
  const tooLongNotesResult = scorePublicationComprehension([tooLongNotes], corpus);
  assert.ok(
    tooLongNotesResult.responses[0].errors.some((error) => error.code === "invalid_notes"),
  );

  const maxNotes = responseFor({
    caseId: "complete",
    track: "html",
    evaluatorKind: "zero_context_ai",
    runId: "notes-max-length",
  });
  maxNotes.notes = "n".repeat(4000);
  const maxNotesResult = scorePublicationComprehension([maxNotes], corpus);
  assert.ok(!maxNotesResult.responses[0].errors.some((error) => error.code === "invalid_notes"));
}

function testUnknownResponseAndSchemaProperties() {
  const unknownResponseProperty = responseFor({
    caseId: "complete",
    track: "html",
    evaluatorKind: "zero_context_ai",
    runId: "unknown-response-property",
  });
  unknownResponseProperty.unexpected_top_level = true;
  const unknownResponsePropertyResult = scorePublicationComprehension(
    [unknownResponseProperty],
    corpus,
  );
  assert.ok(
    unknownResponsePropertyResult.responses[0].errors.some(
      (error) => error.code === "unknown_response_property" && error.property === "unexpected_top_level",
    ),
  );

  const unknownSchemaProperty = responseFor({
    caseId: "complete",
    track: "html",
    evaluatorKind: "zero_context_ai",
    runId: "unknown-schema-property",
  });
  unknownSchemaProperty.schema.extra = "value";
  const unknownSchemaPropertyResult = scorePublicationComprehension(
    [unknownSchemaProperty],
    corpus,
  );
  assert.ok(
    unknownSchemaPropertyResult.responses[0].errors.some(
      (error) => error.code === "unknown_schema_property" && error.property === "extra",
    ),
  );
}

function testCliExitBehavior() {
  const helpRun = spawnSync(process.execPath, [scorerScriptPath, "--help"], {
    encoding: "utf8",
  });
  assert.equal(helpRun.status, 0, `--help stderr: ${helpRun.stderr}`);
  assert.match(helpRun.stdout, /^Usage: node scripts\/score_publication_comprehension\.mjs/);

  const noArgsRun = spawnSync(process.execPath, [scorerScriptPath], { encoding: "utf8" });
  assert.equal(noArgsRun.status, 2, `no-args stderr: ${noArgsRun.stderr}`);
  assert.match(noArgsRun.stderr, /^Usage: node scripts\/score_publication_comprehension\.mjs/);

  const missingFileRun = spawnSync(
    process.execPath,
    [scorerScriptPath, path.join(tmpdir(), "publication-comprehension-does-not-exist.json")],
    { encoding: "utf8" },
  );
  assert.equal(missingFileRun.status, 1, `missing-file stderr: ${missingFileRun.stderr}`);
  const missingFileOutput = JSON.parse(missingFileRun.stdout);
  assert.equal(missingFileOutput.status, "invalid");
  assert.ok(
    missingFileOutput.responses[0].errors.some((error) => error.code === "response_file_unreadable"),
  );

  const brokenJsonDir = mkdtempSync(path.join(tmpdir(), "publication-comprehension-broken-json-"));
  try {
    const brokenJsonPath = path.join(brokenJsonDir, "broken.json");
    writeFileSync(brokenJsonPath, "{ not valid json");
    const brokenJsonRun = spawnSync(process.execPath, [scorerScriptPath, brokenJsonPath], {
      encoding: "utf8",
    });
    assert.equal(brokenJsonRun.status, 1, `broken-json stderr: ${brokenJsonRun.stderr}`);
    const brokenJsonOutput = JSON.parse(brokenJsonRun.stdout);
    assert.equal(brokenJsonOutput.status, "invalid");
    assert.ok(
      brokenJsonOutput.responses[0].errors.some((error) => error.code === "response_json_invalid"),
    );
  } finally {
    rmSync(brokenJsonDir, { recursive: true, force: true });
  }

  const validResponseDir = mkdtempSync(path.join(tmpdir(), "publication-comprehension-valid-response-"));
  try {
    const validResponsePath = path.join(validResponseDir, "response.json");
    const validResponse = responseFor({
      caseId: "complete",
      track: "html",
      evaluatorKind: "zero_context_ai",
      runId: "cli-single-valid-run",
    });
    writeFileSync(validResponsePath, JSON.stringify(validResponse, null, 2));
    const validRun = spawnSync(process.execPath, [scorerScriptPath, validResponsePath], {
      encoding: "utf8",
    });
    // A single valid run is below the minimum distinct runs, so the group
    // (and therefore the overall report) is not_run. not_run must still
    // exit 0: it is not a scorer failure.
    assert.equal(validRun.status, 0, `valid-single-run stderr: ${validRun.stderr}`);
    const validRunOutput = JSON.parse(validRun.stdout);
    assert.equal(validRunOutput.status, "not_run");
  } finally {
    rmSync(validResponseDir, { recursive: true, force: true });
  }
}

function testAllGroupsPassAndOutputIsDeterministic() {
  const responses = [];
  for (const caseId of corpus.cases) {
    responses.push(...aiGroupResponses(caseId, "json"));
    responses.push(...aiGroupResponses(caseId, "html"));
    responses.push(...humanGroupResponses(caseId));
  }
  const records = responses.map((response, index) => ({
    source: `fixture:${index + 1}`,
    response,
  }));
  const forward = scorePublicationComprehensionRecords(records, corpus);
  const reverse = scorePublicationComprehensionRecords([...records].reverse(), corpus);
  assert.equal(forward.schema.name, "org.synapsegit.publication-comprehension-score-report");
  assert.equal(forward.status, "pass");
  assert.ok(forward.groups.every((group) => group.status === "pass"));
  assert.deepEqual(reverse, forward);
}

function testMissingAndWrongQuestionTypesAreIncorrect() {
  const responses = aiGroupResponses("complete", "html", (response, index) => {
    if (index === 0) delete response.answers.P01;
    if (index === 1) response.answers.P01 = "3";
  });
  const result = scorePublicationComprehension(responses, corpus);
  const group = findGroup(result, "complete", "html", "zero_context_ai");
  assert.equal(group.status, "pass");
  const scored = result.responses.filter(
    (response) => response.case_id === "complete" && response.track === "html",
  );
  assert.equal(scored[0].valid, true);
  assert.equal(scored[0].score.correct, 13);
  assert.equal(scored[1].valid, true);
  assert.equal(scored[1].score.correct, 13);
  assert.deepEqual(scored[0].score.incorrect_question_ids, ["P01"]);
  assert.deepEqual(scored[1].score.incorrect_question_ids, ["P01"]);
}

function testAiAccuracyAndCriticalGates() {
  const belowAccuracy = aiGroupResponses("complete", "html", (response, index) => {
    if (index === 0) {
      response.answers.P01 = 0;
      response.answers.P02 = 1;
    }
  });
  const accuracyResult = scorePublicationComprehension(belowAccuracy, corpus);
  const accuracyGroup = findGroup(
    accuracyResult,
    "complete",
    "html",
    "zero_context_ai",
  );
  assert.equal(accuracyGroup.status, "fail");
  assert.deepEqual(accuracyGroup.reasons, ["run_accuracy_below_threshold"]);
  assert.equal(accuracyResult.status, "not_run");

  const criticalMiss = aiGroupResponses("complete", "html", (response, index) => {
    if (index === 0) response.answers.P04 = true;
  });
  const criticalResult = scorePublicationComprehension(criticalMiss, corpus);
  const criticalGroup = findGroup(
    criticalResult,
    "complete",
    "html",
    "zero_context_ai",
  );
  assert.equal(criticalGroup.status, "fail");
  assert.deepEqual(criticalGroup.reasons, ["critical_answer_incorrect"]);
}

function testHumanMacroAndCriticalThresholds() {
  const exactMacro = humanGroupResponses("complete", (response, index) => {
    if (index < 7) {
      response.answers.P01 = 0;
      response.answers.P02 = 1;
      response.answers.P03 = true;
    }
  });
  const exactResult = scorePublicationComprehension(exactMacro, corpus);
  const exactGroup = findGroup(exactResult, "complete", "html", "human");
  assert.deepEqual(exactGroup.macro_accuracy, { numerator: 119, denominator: 140 });
  assert.equal(exactGroup.status, "pass");

  const belowMacro = humanGroupResponses("complete", (response, index) => {
    if (index < 7) {
      response.answers.P01 = 0;
      response.answers.P02 = 1;
      response.answers.P03 = true;
    } else if (index === 7) {
      response.answers.P01 = 0;
    }
  });
  const belowResult = scorePublicationComprehension(belowMacro, corpus);
  const belowGroup = findGroup(belowResult, "complete", "html", "human");
  assert.deepEqual(belowGroup.macro_accuracy, { numerator: 118, denominator: 140 });
  assert.equal(belowGroup.status, "fail");
  assert.ok(belowGroup.reasons.includes("macro_accuracy_below_threshold"));

  const exactCritical = humanGroupResponses("complete", (response, index) => {
    if (index === 0) response.answers.P04 = true;
  });
  const exactCriticalResult = scorePublicationComprehension(exactCritical, corpus);
  const exactCriticalGroup = findGroup(exactCriticalResult, "complete", "html", "human");
  const p04Exact = exactCriticalGroup.critical_questions.find(
    (question) => question.question_id === "P04",
  );
  assert.equal(p04Exact.correct_participants, 9);
  assert.equal(p04Exact.meets_threshold, true);
  assert.equal(exactCriticalGroup.status, "pass");

  const belowCritical = humanGroupResponses("complete", (response, index) => {
    if (index < 2) response.answers.P04 = true;
  });
  const belowCriticalResult = scorePublicationComprehension(belowCritical, corpus);
  const belowCriticalGroup = findGroup(
    belowCriticalResult,
    "complete",
    "html",
    "human",
  );
  assert.equal(belowCriticalGroup.status, "fail");
  assert.ok(belowCriticalGroup.reasons.includes("critical_question_accuracy_below_threshold"));
}

function testValidationAndStatusSemantics() {
  const unknown = responseFor({
    caseId: "complete",
    track: "html",
    evaluatorKind: "zero_context_ai",
    runId: "unknown-question",
  });
  unknown.answers.Z99 = false;
  const unknownResult = scorePublicationComprehension([unknown], corpus);
  assert.equal(unknownResult.status, "invalid");
  assert.equal(findGroup(unknownResult, "complete", "html", "zero_context_ai").status, "invalid");
  assert.ok(unknownResult.responses[0].errors.some((error) => error.code === "unknown_question_id"));
  assert.deepEqual(unknownResult.errors, []);

  const inapplicable = responseFor({
    caseId: "incomplete-only",
    track: "html",
    evaluatorKind: "zero_context_ai",
    runId: "inapplicable-question",
  });
  inapplicable.answers.I06 = false;
  const inapplicableResult = scorePublicationComprehension([inapplicable], corpus);
  assert.equal(
    findGroup(inapplicableResult, "incomplete-only", "html", "zero_context_ai").status,
    "invalid",
  );
  assert.ok(
    inapplicableResult.responses[0].errors.some(
      (error) => error.code === "inapplicable_question_id",
    ),
  );

  const nonPrimitive = responseFor({
    caseId: "complete",
    track: "html",
    evaluatorKind: "zero_context_ai",
    runId: "non-primitive",
  });
  nonPrimitive.answers.P01 = null;
  const nonPrimitiveResult = scorePublicationComprehension([nonPrimitive], corpus);
  assert.equal(
    findGroup(nonPrimitiveResult, "complete", "html", "zero_context_ai").status,
    "invalid",
  );
  assert.ok(
    nonPrimitiveResult.responses[0].errors.some((error) => error.code === "invalid_answer_value"),
  );

  const digestMismatch = responseFor({
    caseId: "complete",
    track: "json",
    evaluatorKind: "zero_context_ai",
    runId: "digest-mismatch",
  });
  digestMismatch.input_artifact_sha256 = "0".repeat(64);
  const digestResult = scorePublicationComprehension([digestMismatch], corpus);
  assert.equal(findGroup(digestResult, "complete", "json", "zero_context_ai").status, "invalid");
  assert.ok(
    digestResult.responses[0].errors.some(
      (error) => error.code === "input_artifact_sha256_mismatch",
    ),
  );

  const duplicates = aiGroupResponses("complete", "json");
  duplicates[1].run_id = duplicates[0].run_id;
  const duplicateResult = scorePublicationComprehension(duplicates, corpus);
  assert.equal(
    findGroup(duplicateResult, "complete", "json", "zero_context_ai").status,
    "invalid",
  );

  const unassignableResult = scorePublicationComprehension([{}], corpus);
  assert.equal(unassignableResult.status, "invalid");
  assert.ok(unassignableResult.errors.length > 0);

  const incompleteRun = scorePublicationComprehension(
    aiGroupResponses("complete", "json").slice(0, -1),
    corpus,
  );
  assert.equal(
    findGroup(incompleteRun, "complete", "json", "zero_context_ai").status,
    "not_run",
  );
  assert.equal(incompleteRun.status, "not_run");
}

testCorpusAndIntegerThresholds();
testAllGroupsPassAndOutputIsDeterministic();
testMissingAndWrongQuestionTypesAreIncorrect();
testAiAccuracyAndCriticalGates();
testHumanMacroAndCriticalThresholds();
testValidationAndStatusSemantics();
testFrozenCorpusThresholdsRegression();
testCorpusLoadRejectsSchemaIdentityTampering();
testCorpusLoadRejectsTrackMatrixMismatch();
testCorpusLoadRejectsDuplicateCasesAndTracks();
testCorpusLoadRejectsEmptyCaseTrackApplicability();
testCorpusLoadRejectsNonSafeIntegerOracleAnswer();
testEvaluatorMetadataValidation();
testRunIdAndNotesBoundaries();
testUnknownResponseAndSchemaProperties();
testCliExitBehavior();

process.stdout.write("publication_comprehension_scorer_tests_ok\n");
