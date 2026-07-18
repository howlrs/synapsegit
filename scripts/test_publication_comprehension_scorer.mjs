#!/usr/bin/env node

import assert from "node:assert/strict";

import {
  loadPublicationComprehensionCorpus,
  meetsPercentThreshold,
  scorePublicationComprehension,
  scorePublicationComprehensionRecords,
} from "./score_publication_comprehension.mjs";

const corpus = loadPublicationComprehensionCorpus();

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

process.stdout.write("publication_comprehension_scorer_tests_ok\n");
