#!/usr/bin/env node

import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

const scriptPath = fileURLToPath(import.meta.url);
const repositoryRoot = path.resolve(path.dirname(scriptPath), "..");

export const publicationComprehensionCorpusDir = path.join(
  repositoryRoot,
  "docs/evaluation/publication-comprehension/v1",
);

const responseSchemaName = "org.synapsegit.publication-comprehension-response";
const resultSchemaName = "org.synapsegit.publication-comprehension-score-report";
const sha256Pattern = /^[0-9a-f]{64}$/;
const allowedResponseProperties = new Set([
  "schema",
  "corpus_version",
  "case_id",
  "track",
  "evaluator_kind",
  "run_id",
  "input_artifact_sha256",
  "evaluator_metadata",
  "answers",
  "notes",
]);
const groupKinds = Object.freeze([
  Object.freeze({ evaluator_kind: "zero_context_ai", track: "json" }),
  Object.freeze({ evaluator_kind: "zero_context_ai", track: "html" }),
  Object.freeze({ evaluator_kind: "human", track: "html" }),
]);

function isObject(value) {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function stringLength(value) {
  return [...value].length;
}

function compareText(left, right) {
  return left < right ? -1 : left > right ? 1 : 0;
}

function readJson(filePath) {
  return JSON.parse(readFileSync(filePath, "utf8"));
}

function sha256File(filePath) {
  return createHash("sha256").update(readFileSync(filePath)).digest("hex");
}

function requireCondition(condition, message) {
  if (!condition) {
    throw new Error(`publication comprehension corpus error: ${message}`);
  }
}

function requirePositiveInteger(value, label) {
  requireCondition(Number.isSafeInteger(value) && value > 0, `${label} must be a positive integer`);
  return value;
}

function requirePercent(value, label) {
  requireCondition(
    Number.isSafeInteger(value) && value >= 0 && value <= 100,
    `${label} must be an integer from 0 through 100`,
  );
  return value;
}

function expectedValueHasType(value, answerType) {
  if (answerType === "boolean") return typeof value === "boolean";
  if (answerType === "integer") return typeof value === "number" && Number.isInteger(value);
  if (answerType === "enum") return typeof value === "string";
  return false;
}

function answerHasType(value, answerType) {
  return expectedValueHasType(value, answerType);
}

function loadHtmlDigest(corpusDir, caseId, oracleCase) {
  requireCondition(typeof oracleCase.bundle === "string", `oracle case ${caseId} has no bundle path`);
  const bundleDir = path.resolve(corpusDir, oracleCase.bundle);
  const relativeBundle = path.relative(corpusDir, bundleDir);
  requireCondition(
    relativeBundle !== "" && !relativeBundle.startsWith("..") && !path.isAbsolute(relativeBundle),
    `oracle case ${caseId} bundle path escapes the corpus directory`,
  );
  const checksums = readJson(path.join(bundleDir, "checksums.json"));
  requireCondition(checksums.algorithm === "sha256", `${caseId} checksums must use sha256`);
  requireCondition(Array.isArray(checksums.files), `${caseId} checksums files must be an array`);
  const entries = checksums.files.filter((entry) => isObject(entry) && entry.path === "index.html");
  requireCondition(entries.length === 1, `${caseId} checksums must contain exactly one index.html`);
  requireCondition(
    typeof entries[0].sha256 === "string" && sha256Pattern.test(entries[0].sha256),
    `${caseId} index.html checksum is not a lowercase sha256 digest`,
  );
  requireCondition(
    sha256File(path.join(bundleDir, "index.html")) === entries[0].sha256,
    `${caseId} index.html bytes do not match checksums.json`,
  );
  requireCondition(
    sha256File(path.join(bundleDir, "projection.json")) === oracleCase.projection_sha256,
    `${caseId} projection.json bytes do not match the oracle digest`,
  );
  return entries[0].sha256;
}

/**
 * Load and validate the frozen scorer inputs. The CLI always uses the fixed
 * repository path; the argument exists so focused tests can use a fixture.
 */
export function loadPublicationComprehensionCorpus(corpusDir = publicationComprehensionCorpusDir) {
  const questionnaire = readJson(path.join(corpusDir, "questionnaire.json"));
  const oracle = readJson(path.join(corpusDir, "oracle.json"));
  const protocol = readJson(path.join(corpusDir, "protocol.json"));

  requireCondition(isObject(questionnaire), "questionnaire.json must contain an object");
  requireCondition(isObject(oracle), "oracle.json must contain an object");
  requireCondition(isObject(protocol), "protocol.json must contain an object");
  requireCondition(
    Number.isSafeInteger(questionnaire.corpus_version) && questionnaire.corpus_version > 0,
    "questionnaire corpus_version must be a positive integer",
  );
  requireCondition(
    oracle.corpus_version === questionnaire.corpus_version &&
      protocol.corpus_version === questionnaire.corpus_version,
    "questionnaire, oracle, and protocol corpus versions must match",
  );
  requireCondition(Array.isArray(questionnaire.questions), "questionnaire questions must be an array");
  requireCondition(isObject(oracle.cases), "oracle cases must be an object");
  requireCondition(isObject(protocol.zero_context_ai), "protocol zero_context_ai must be an object");
  requireCondition(isObject(protocol.human), "protocol human must be an object");

  const cases = Object.keys(oracle.cases).sort();
  requireCondition(cases.length > 0, "oracle must define at least one case");
  const caseSet = new Set(cases);
  const questionsById = new Map();

  for (const question of questionnaire.questions) {
    requireCondition(isObject(question), "every questionnaire entry must be an object");
    requireCondition(
      typeof question.id === "string" && question.id.length > 0,
      "every questionnaire entry must have a non-empty id",
    );
    requireCondition(!questionsById.has(question.id), `question ${question.id} is duplicated`);
    requireCondition(
      ["boolean", "integer", "enum"].includes(question.answer_type),
      `question ${question.id} has an unsupported answer_type`,
    );
    requireCondition(typeof question.critical === "boolean", `question ${question.id} needs critical`);
    requireCondition(
      Array.isArray(question.cases) && question.cases.length > 0,
      `question ${question.id} must name at least one case`,
    );
    requireCondition(
      question.cases.every((caseId) => typeof caseId === "string" && caseSet.has(caseId)),
      `question ${question.id} names an unknown case`,
    );
    requireCondition(
      Array.isArray(question.tracks) &&
        question.tracks.length > 0 &&
        question.tracks.every((track) => ["json", "html"].includes(track)),
      `question ${question.id} must name supported tracks`,
    );
    if (question.answer_type === "enum") {
      requireCondition(
        Array.isArray(question.accepted_values) &&
          question.accepted_values.length > 0 &&
          question.accepted_values.every((value) => typeof value === "string"),
        `enum question ${question.id} must define string accepted_values`,
      );
    }
    questionsById.set(question.id, question);
  }

  const questionsByCaseTrack = new Map();
  const artifactDigests = new Map();
  for (const caseId of cases) {
    const oracleCase = oracle.cases[caseId];
    requireCondition(isObject(oracleCase), `oracle case ${caseId} must be an object`);
    requireCondition(isObject(oracleCase.answers), `oracle case ${caseId} answers must be an object`);
    requireCondition(
      typeof oracleCase.projection_sha256 === "string" &&
        sha256Pattern.test(oracleCase.projection_sha256),
      `oracle case ${caseId} projection_sha256 is invalid`,
    );

    const applicable = questionnaire.questions.filter((question) => question.cases.includes(caseId));
    const applicableIds = new Set(applicable.map((question) => question.id));
    requireCondition(
      Object.keys(oracleCase.answers).length === applicable.length,
      `oracle case ${caseId} answer count does not match its questionnaire`,
    );

    const scoredQuestions = applicable.map((question) => {
      const oracleAnswer = oracleCase.answers[question.id];
      requireCondition(isObject(oracleAnswer), `oracle case ${caseId} lacks ${question.id}`);
      requireCondition(
        oracleAnswer.critical === question.critical,
        `oracle and questionnaire critical flags differ for ${caseId}/${question.id}`,
      );
      requireCondition(
        expectedValueHasType(oracleAnswer.value, question.answer_type),
        `oracle answer ${caseId}/${question.id} has the wrong primitive type`,
      );
      if (question.answer_type === "enum") {
        requireCondition(
          question.accepted_values.includes(oracleAnswer.value),
          `oracle answer ${caseId}/${question.id} is outside accepted_values`,
        );
      }
      return Object.freeze({
        id: question.id,
        answer_type: question.answer_type,
        critical: question.critical,
        expected: oracleAnswer.value,
      });
    });

    for (const answerId of Object.keys(oracleCase.answers)) {
      requireCondition(applicableIds.has(answerId), `oracle case ${caseId} has unexpected ${answerId}`);
    }

    for (const track of ["json", "html"]) {
      questionsByCaseTrack.set(
        `${caseId}\u0000${track}`,
        Object.freeze(
          scoredQuestions.filter((question) =>
            questionsById.get(question.id).tracks.includes(track),
          ),
        ),
      );
    }
    artifactDigests.set(
      `${caseId}\u0000json`,
      oracleCase.projection_sha256,
    );
    artifactDigests.set(
      `${caseId}\u0000html`,
      loadHtmlDigest(corpusDir, caseId, oracleCase),
    );
  }

  const thresholds = Object.freeze({
    aiMinimumRuns: requirePositiveInteger(
      protocol.zero_context_ai.minimum_runs_per_case_and_track,
      "zero_context_ai.minimum_runs_per_case_and_track",
    ),
    aiJsonMinimumPercent: requirePercent(
      protocol.zero_context_ai.json_track_minimum_accuracy_percent,
      "zero_context_ai.json_track_minimum_accuracy_percent",
    ),
    aiHtmlMinimumPercent: requirePercent(
      protocol.zero_context_ai.html_track_minimum_accuracy_percent,
      "zero_context_ai.html_track_minimum_accuracy_percent",
    ),
    aiAllCriticalRequired: protocol.zero_context_ai.all_critical_answers_must_be_correct === true,
    humanMinimumParticipants: requirePositiveInteger(
      protocol.human.minimum_participants_per_case,
      "human.minimum_participants_per_case",
    ),
    humanMinimumMacroPercent: requirePercent(
      protocol.human.minimum_macro_accuracy_percent,
      "human.minimum_macro_accuracy_percent",
    ),
    humanCriticalMinimumPercent: requirePercent(
      protocol.human.minimum_accuracy_per_critical_question_percent,
      "human.minimum_accuracy_per_critical_question_percent",
    ),
    humanAllCriticalRequired: protocol.human.all_critical_questions_must_meet_threshold === true,
  });

  requireCondition(
    thresholds.aiAllCriticalRequired,
    "zero_context_ai.all_critical_answers_must_be_correct must be true",
  );
  requireCondition(
    thresholds.humanAllCriticalRequired,
    "human.all_critical_questions_must_meet_threshold must be true",
  );

  return Object.freeze({
    corpusVersion: questionnaire.corpus_version,
    cases: Object.freeze(cases),
    caseSet,
    questionsById,
    questionsByCaseTrack,
    artifactDigests,
    thresholds,
  });
}

function issue(code, message, extra = undefined) {
  return extra === undefined ? { code, message } : { code, message, ...extra };
}

function validGroupKey(response, corpus) {
  if (!isObject(response) || !corpus.caseSet.has(response.case_id)) return null;
  if (response.evaluator_kind === "zero_context_ai" && ["json", "html"].includes(response.track)) {
    return `${response.case_id}\u0000${response.track}\u0000zero_context_ai`;
  }
  if (response.evaluator_kind === "human" && response.track === "html") {
    return `${response.case_id}\u0000html\u0000human`;
  }
  return null;
}

function scoreAnswers(response, questions) {
  const answers = isObject(response?.answers) ? response.answers : {};
  const questionResults = [];
  let correct = 0;
  let criticalCorrect = 0;
  let criticalTotal = 0;

  for (const question of questions) {
    const present = Object.prototype.hasOwnProperty.call(answers, question.id);
    const actual = present ? answers[question.id] : undefined;
    const typeCorrect = present && answerHasType(actual, question.answer_type);
    const answerCorrect = typeCorrect && actual === question.expected;
    if (answerCorrect) correct += 1;
    if (question.critical) {
      criticalTotal += 1;
      if (answerCorrect) criticalCorrect += 1;
    }
    questionResults.push({
      question_id: question.id,
      critical: question.critical,
      correct: answerCorrect,
      ...(present ? { answer_type_correct: typeCorrect } : { missing: true }),
    });
  }

  return {
    correct,
    total: questions.length,
    accuracy: { numerator: correct, denominator: questions.length },
    critical_correct: criticalCorrect,
    critical_total: criticalTotal,
    all_critical_correct: criticalCorrect === criticalTotal,
    incorrect_question_ids: questionResults
      .filter((result) => !result.correct)
      .map((result) => result.question_id),
    question_results: questionResults,
  };
}

function validateResponse(response, corpus) {
  const errors = [];
  if (!isObject(response)) {
    return [issue("response_not_object", "response JSON must contain an object")];
  }

  for (const property of Object.keys(response).sort()) {
    if (!allowedResponseProperties.has(property)) {
      errors.push(issue("unknown_response_property", `unknown response property: ${property}`, { property }));
    }
  }

  if (
    !isObject(response.schema) ||
    response.schema.name !== responseSchemaName ||
    response.schema.version !== 1
  ) {
    errors.push(issue("invalid_schema", `schema must identify ${responseSchemaName} version 1`));
  } else {
    for (const property of Object.keys(response.schema).sort()) {
      if (!["name", "version"].includes(property)) {
        errors.push(issue("unknown_schema_property", `unknown schema property: ${property}`, { property }));
      }
    }
  }
  if (response.corpus_version !== corpus.corpusVersion) {
    errors.push(
      issue("invalid_corpus_version", `corpus_version must be ${corpus.corpusVersion}`),
    );
  }
  if (!corpus.caseSet.has(response.case_id)) {
    errors.push(issue("invalid_case_id", "case_id is not defined by the oracle"));
  }
  if (!["json", "html"].includes(response.track)) {
    errors.push(issue("invalid_track", "track must be json or html"));
  }
  if (!["zero_context_ai", "human"].includes(response.evaluator_kind)) {
    errors.push(
      issue("invalid_evaluator_kind", "evaluator_kind must be zero_context_ai or human"),
    );
  } else if (response.evaluator_kind === "human" && response.track !== "html") {
    errors.push(issue("invalid_human_track", "human responses are allowed only on the html track"));
  }
  if (
    typeof response.run_id !== "string" ||
    stringLength(response.run_id) < 1 ||
    stringLength(response.run_id) > 128
  ) {
    errors.push(issue("invalid_run_id", "run_id must be a string of 1 through 128 characters"));
  }
  if (!isObject(response.evaluator_metadata)) {
    errors.push(issue("invalid_evaluator_metadata", "evaluator_metadata must be an object"));
  } else {
    const metadataLimits = new Map([
      ["model_id", 256],
      ["model_configuration", 2000],
      ["browser_id", 256],
      ["assistive_technology", 256],
    ]);
    for (const property of Object.keys(response.evaluator_metadata).sort()) {
      if (!metadataLimits.has(property)) {
        errors.push(
          issue("unknown_evaluator_metadata_property", `unknown evaluator_metadata property: ${property}`, {
            property,
          }),
        );
        continue;
      }
      const value = response.evaluator_metadata[property];
      if (
        typeof value !== "string" ||
        stringLength(value) < 1 ||
        stringLength(value) > metadataLimits.get(property)
      ) {
        errors.push(
          issue(
            "invalid_evaluator_metadata_property",
            `evaluator_metadata.${property} must be a non-empty bounded string`,
            { property },
          ),
        );
      }
    }
    if (response.evaluator_kind === "zero_context_ai") {
      for (const property of ["model_id", "model_configuration"]) {
        if (!Object.prototype.hasOwnProperty.call(response.evaluator_metadata, property)) {
          errors.push(
            issue(
              "missing_evaluator_metadata_property",
              `zero_context_ai requires evaluator_metadata.${property}`,
              { property },
            ),
          );
        }
      }
    } else if (
      response.evaluator_kind === "human" &&
      !Object.prototype.hasOwnProperty.call(response.evaluator_metadata, "browser_id")
    ) {
      errors.push(
        issue(
          "missing_evaluator_metadata_property",
          "human requires evaluator_metadata.browser_id",
          { property: "browser_id" },
        ),
      );
    }
  }
  if (!isObject(response.answers)) {
    errors.push(issue("invalid_answers", "answers must be an object"));
  } else {
    const answerEntries = Object.entries(response.answers);
    if (answerEntries.length === 0) {
      errors.push(issue("invalid_answers", "answers must contain at least one property"));
    }
    for (const [questionId, value] of answerEntries.sort(([left], [right]) =>
      compareText(left, right),
    )) {
      const validPrimitive =
        typeof value === "boolean" ||
        (typeof value === "number" && Number.isSafeInteger(value)) ||
        (typeof value === "string" && stringLength(value) >= 1 && stringLength(value) <= 256);
      if (!validPrimitive) {
        errors.push(
          issue(
            "invalid_answer_value",
            `answer ${questionId} must be a boolean, safe integer, or non-empty bounded string`,
            { question_id: questionId },
          ),
        );
      }
    }
  }
  if (Object.prototype.hasOwnProperty.call(response, "notes")) {
    if (typeof response.notes !== "string" || stringLength(response.notes) > 4000) {
      errors.push(issue("invalid_notes", "notes must be a string of at most 4000 characters"));
    }
  }

  if (
    typeof response.input_artifact_sha256 !== "string" ||
    !sha256Pattern.test(response.input_artifact_sha256)
  ) {
    errors.push(
      issue(
        "invalid_input_artifact_sha256",
        "input_artifact_sha256 is required and must be a lowercase sha256 digest",
      ),
    );
  } else if (corpus.caseSet.has(response.case_id) && ["json", "html"].includes(response.track)) {
    const expectedDigest = corpus.artifactDigests.get(`${response.case_id}\u0000${response.track}`);
    if (response.input_artifact_sha256 !== expectedDigest) {
      errors.push(
        issue(
          "input_artifact_sha256_mismatch",
          `input_artifact_sha256 does not match ${response.case_id}/${response.track}`,
        ),
      );
    }
  }

  if (
    isObject(response.answers) &&
    corpus.caseSet.has(response.case_id) &&
    ["json", "html"].includes(response.track)
  ) {
    const applicableIds = new Set(
      corpus.questionsByCaseTrack
        .get(`${response.case_id}\u0000${response.track}`)
        .map((question) => question.id),
    );
    for (const questionId of Object.keys(response.answers).sort()) {
      if (!corpus.questionsById.has(questionId)) {
        errors.push(
          issue("unknown_question_id", `unknown question id: ${questionId}`, {
            question_id: questionId,
          }),
        );
      } else if (!applicableIds.has(questionId)) {
        errors.push(
          issue(
            "inapplicable_question_id",
            `question ${questionId} does not apply to ${response.case_id}/${response.track}`,
            { question_id: questionId },
          ),
        );
      }
    }
  }
  return errors;
}

function safeString(value) {
  return typeof value === "string" ? value : null;
}

function safeEvaluatorMetadata(value) {
  if (!isObject(value)) return null;
  const metadata = {};
  for (const property of [
    "model_id",
    "model_configuration",
    "browser_id",
    "assistive_technology",
  ]) {
    if (typeof value[property] === "string") metadata[property] = value[property];
  }
  return metadata;
}

function evaluateRecord(input, corpus, index) {
  const source = typeof input?.source === "string" ? input.source : `response:${index + 1}`;
  const loadErrors = Array.isArray(input?.load_errors) ? input.load_errors : [];
  const response = input?.response;
  const errors = [...loadErrors, ...(loadErrors.length === 0 ? validateResponse(response, corpus) : [])];
  const groupKey = loadErrors.length === 0 ? validGroupKey(response, corpus) : null;
  const questions =
    isObject(response) &&
    corpus.caseSet.has(response.case_id) &&
    ["json", "html"].includes(response.track)
    ? corpus.questionsByCaseTrack.get(`${response.case_id}\u0000${response.track}`)
    : null;
  const score = questions === null ? null : scoreAnswers(response, questions);

  return {
    groupKey,
    originalIndex: index,
    output: {
      source,
      case_id: safeString(response?.case_id),
      track: safeString(response?.track),
      evaluator_kind: safeString(response?.evaluator_kind),
      run_id: safeString(response?.run_id),
      input_artifact_sha256: safeString(response?.input_artifact_sha256),
      evaluator_metadata: safeEvaluatorMetadata(response?.evaluator_metadata),
      valid: errors.length === 0,
      errors,
      score,
    },
  };
}

/** Compare an integer ratio to an integer percent without rounding. */
export function meetsPercentThreshold(numerator, denominator, percent) {
  if (
    !Number.isSafeInteger(numerator) ||
    !Number.isSafeInteger(denominator) ||
    !Number.isSafeInteger(percent) ||
    numerator < 0 ||
    denominator <= 0 ||
    numerator > denominator ||
    percent < 0 ||
    percent > 100
  ) {
    throw new TypeError("threshold operands must be bounded safe integers");
  }
  return BigInt(numerator) * 100n >= BigInt(denominator) * BigInt(percent);
}

function duplicateRunIds(records) {
  const counts = new Map();
  for (const record of records) {
    const runId = record.output.run_id;
    if (runId !== null && runId.length > 0) counts.set(runId, (counts.get(runId) ?? 0) + 1);
  }
  return [...counts]
    .filter(([, count]) => count > 1)
    .map(([runId]) => runId)
    .sort();
}

function aiGroup(caseId, track, records, corpus) {
  const threshold = track === "json"
    ? corpus.thresholds.aiJsonMinimumPercent
    : corpus.thresholds.aiHtmlMinimumPercent;
  const distinctRunIds = new Set(
    records.map((record) => record.output.run_id).filter((runId) => runId !== null && runId.length > 0),
  );
  const invalidCount = records.filter((record) => !record.output.valid).length;
  const duplicates = duplicateRunIds(records);
  const belowAccuracy = records.filter(
    (record) =>
      record.output.score !== null &&
      !meetsPercentThreshold(
        record.output.score.correct,
        record.output.score.total,
        threshold,
      ),
  );
  const criticalFailures = records.filter(
    (record) => record.output.score !== null && !record.output.score.all_critical_correct,
  );
  const reasons = [];
  let status;
  if (records.length === 0) {
    status = "not_run";
    reasons.push("no_responses");
  } else if (invalidCount > 0 || duplicates.length > 0) {
    status = "invalid";
    if (invalidCount > 0) reasons.push("invalid_responses");
    if (duplicates.length > 0) reasons.push("duplicate_run_ids");
  } else if (distinctRunIds.size < corpus.thresholds.aiMinimumRuns) {
    status = "not_run";
    reasons.push("insufficient_distinct_runs");
  } else if (belowAccuracy.length > 0 || criticalFailures.length > 0) {
    status = "fail";
    if (belowAccuracy.length > 0) reasons.push("run_accuracy_below_threshold");
    if (criticalFailures.length > 0) reasons.push("critical_answer_incorrect");
  } else {
    status = "pass";
  }

  return {
    case_id: caseId,
    track,
    evaluator_kind: "zero_context_ai",
    status,
    reasons,
    response_count: records.length,
    invalid_response_count: invalidCount,
    required_distinct_runs: corpus.thresholds.aiMinimumRuns,
    distinct_runs: distinctRunIds.size,
    minimum_accuracy_percent: threshold,
    all_critical_answers_required: true,
    duplicate_run_ids: duplicates,
    below_accuracy_sources: belowAccuracy.map((record) => record.output.source),
    critical_failure_sources: criticalFailures.map((record) => record.output.source),
  };
}

function humanGroup(caseId, records, corpus) {
  const duplicates = duplicateRunIds(records);
  const participantRecords = [];
  const seen = new Set();
  for (const record of records) {
    const runId = record.output.run_id;
    if (runId !== null && runId.length > 0 && !seen.has(runId)) {
      seen.add(runId);
      participantRecords.push(record);
    }
  }
  const invalidCount = records.filter((record) => !record.output.valid).length;
  const questions = corpus.questionsByCaseTrack.get(`${caseId}\u0000html`);
  const totalCorrect = participantRecords.reduce(
    (sum, record) => sum + (record.output.score?.correct ?? 0),
    0,
  );
  const macroDenominator = participantRecords.length * questions.length;
  const macroMeetsThreshold = macroDenominator > 0 && meetsPercentThreshold(
    totalCorrect,
    macroDenominator,
    corpus.thresholds.humanMinimumMacroPercent,
  );
  const criticalQuestions = questions
    .filter((question) => question.critical)
    .map((question) => {
      const correctParticipants = participantRecords.filter((record) => {
        const answer = record.output.score?.question_results.find(
          (result) => result.question_id === question.id,
        );
        return answer?.correct === true;
      }).length;
      const meetsThreshold = participantRecords.length > 0 && meetsPercentThreshold(
        correctParticipants,
        participantRecords.length,
        corpus.thresholds.humanCriticalMinimumPercent,
      );
      return {
        question_id: question.id,
        correct_participants: correctParticipants,
        participants: participantRecords.length,
        minimum_accuracy_percent: corpus.thresholds.humanCriticalMinimumPercent,
        meets_threshold: meetsThreshold,
      };
    });
  const allCriticalMeetThreshold = criticalQuestions.every((question) => question.meets_threshold);
  const reasons = [];
  let status;
  if (records.length === 0) {
    status = "not_run";
    reasons.push("no_responses");
  } else if (invalidCount > 0 || duplicates.length > 0) {
    status = "invalid";
    if (invalidCount > 0) reasons.push("invalid_responses");
    if (duplicates.length > 0) reasons.push("duplicate_run_ids");
  } else if (participantRecords.length < corpus.thresholds.humanMinimumParticipants) {
    status = "not_run";
    reasons.push("insufficient_distinct_participants");
  } else if (!macroMeetsThreshold || !allCriticalMeetThreshold) {
    status = "fail";
    if (!macroMeetsThreshold) reasons.push("macro_accuracy_below_threshold");
    if (!allCriticalMeetThreshold) reasons.push("critical_question_accuracy_below_threshold");
  } else {
    status = "pass";
  }

  return {
    case_id: caseId,
    track: "html",
    evaluator_kind: "human",
    status,
    reasons,
    response_count: records.length,
    invalid_response_count: invalidCount,
    required_distinct_participants: corpus.thresholds.humanMinimumParticipants,
    distinct_participants: participantRecords.length,
    macro_accuracy: { numerator: totalCorrect, denominator: macroDenominator },
    minimum_macro_accuracy_percent: corpus.thresholds.humanMinimumMacroPercent,
    macro_accuracy_meets_threshold: macroMeetsThreshold,
    critical_questions: criticalQuestions,
    all_critical_questions_meet_threshold: allCriticalMeetThreshold,
    duplicate_run_ids: duplicates,
  };
}

function groupSortRank(groupKey, corpus) {
  if (groupKey === null) return Number.MAX_SAFE_INTEGER;
  const [caseId, track, evaluatorKind] = groupKey.split("\u0000");
  const caseIndex = corpus.cases.indexOf(caseId);
  const kindIndex = groupKinds.findIndex(
    (kind) => kind.track === track && kind.evaluator_kind === evaluatorKind,
  );
  return caseIndex * groupKinds.length + kindIndex;
}

function compareRecords(left, right, corpus) {
  const rankDifference = groupSortRank(left.groupKey, corpus) - groupSortRank(right.groupKey, corpus);
  if (rankDifference !== 0) return rankDifference;
  const runDifference = compareText(left.output.run_id ?? "", right.output.run_id ?? "");
  if (runDifference !== 0) return runDifference;
  const sourceDifference = compareText(left.output.source, right.output.source);
  return sourceDifference !== 0 ? sourceDifference : left.originalIndex - right.originalIndex;
}

/**
 * Score already-loaded response records. Each record has `{source, response}`;
 * the CLI additionally uses `load_errors` for unreadable or malformed JSON.
 */
export function scorePublicationComprehensionRecords(inputRecords, corpus) {
  if (!Array.isArray(inputRecords)) throw new TypeError("inputRecords must be an array");
  const evaluated = inputRecords.map((input, index) => evaluateRecord(input, corpus, index));
  evaluated.sort((left, right) => compareRecords(left, right, corpus));

  const recordsByGroup = new Map();
  for (const caseId of corpus.cases) {
    for (const kind of groupKinds) {
      recordsByGroup.set(`${caseId}\u0000${kind.track}\u0000${kind.evaluator_kind}`, []);
    }
  }
  for (const record of evaluated) {
    if (record.groupKey !== null) recordsByGroup.get(record.groupKey).push(record);
  }

  const groups = [];
  for (const caseId of corpus.cases) {
    for (const kind of groupKinds) {
      const key = `${caseId}\u0000${kind.track}\u0000${kind.evaluator_kind}`;
      const records = recordsByGroup.get(key);
      groups.push(
        kind.evaluator_kind === "human"
          ? humanGroup(caseId, records, corpus)
          : aiGroup(caseId, kind.track, records, corpus),
      );
    }
  }

  const unassignedInvalid = evaluated.filter(
    (record) => record.groupKey === null && !record.output.valid,
  );
  const errors = unassignedInvalid.flatMap((record) =>
    record.output.errors.map((error) => ({ source: record.output.source, ...error })),
  );
  let status;
  if (errors.length > 0 || groups.some((group) => group.status === "invalid")) {
    status = "invalid";
  } else if (groups.some((group) => group.status === "not_run")) {
    status = "not_run";
  } else if (groups.some((group) => group.status === "fail")) {
    status = "fail";
  } else {
    status = "pass";
  }

  return {
    schema: { name: resultSchemaName, version: 1 },
    corpus_version: corpus.corpusVersion,
    status,
    responses: evaluated.map((record) => record.output),
    groups,
    errors,
  };
}

/** Convenience wrapper for in-memory response objects. */
export function scorePublicationComprehension(responses, corpus = loadPublicationComprehensionCorpus()) {
  if (!Array.isArray(responses)) throw new TypeError("responses must be an array");
  return scorePublicationComprehensionRecords(
    responses.map((response, index) => ({ source: `response:${index + 1}`, response })),
    corpus,
  );
}

function loadResponseFiles(filePaths) {
  return filePaths.map((filePath, index) => {
    const source = `response:${index + 1}`;
    let text;
    try {
      text = readFileSync(filePath, "utf8");
    } catch (error) {
      return {
        source,
        response: null,
        load_errors: [
          issue("response_file_unreadable", `unable to read response file (${error.code ?? error.name})`),
        ],
      };
    }
    try {
      return { source, response: JSON.parse(text) };
    } catch (error) {
      return {
        source,
        response: null,
        load_errors: [issue("response_json_invalid", `response file is not valid JSON (${error.message})`)],
      };
    }
  });
}

export function runPublicationComprehensionScorer(argv = process.argv.slice(2)) {
  if (argv.length === 1 && ["--help", "-h"].includes(argv[0])) {
    process.stdout.write("Usage: node scripts/score_publication_comprehension.mjs <response.json>...\n");
    return 0;
  }
  if (argv.length === 0) {
    process.stderr.write("Usage: node scripts/score_publication_comprehension.mjs <response.json>...\n");
    return 2;
  }
  const corpus = loadPublicationComprehensionCorpus();
  const result = scorePublicationComprehensionRecords(loadResponseFiles(argv), corpus);
  process.stdout.write(`${JSON.stringify(result, null, 2)}\n`);
  return ["fail", "invalid"].includes(result.status) ? 1 : 0;
}

if (process.argv[1] && path.resolve(process.argv[1]) === scriptPath) {
  try {
    process.exitCode = runPublicationComprehensionScorer();
  } catch (error) {
    process.stderr.write(`score_publication_comprehension_error: ${error.message}\n`);
    process.exitCode = 1;
  }
}
