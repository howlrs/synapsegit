import { archiveTest, inboxTest, expect } from "./fixtures.mjs";

// HTML compiles `pattern` with the `v` flag, where an unescaped trailing `-` in
// a character class is a syntax error and the browser silently ignores the
// attribute. These checks observe the browser's own constraint validation.
const SLUG_PATTERN = "[a-z][a-z0-9\\-]{0,63}";
const EXPECTED_MISMATCH = { ABC: true, "-x": true, a_b: true, "ok-name": false, a: false };

function collectPatternErrors(page) {
  const errors = [];
  page.on("console", (message) => {
    if (message.type() === "error" && /pattern/iu.test(message.text())) errors.push(message.text());
  });
  return errors;
}

async function slugInputs(page) {
  return page.locator("input[pattern]").evaluateAll((inputs, values) => inputs.map((input) => {
    const mismatch = {};
    for (const value of values) {
      input.value = value;
      mismatch[value] = input.validity.patternMismatch;
    }
    input.value = "";
    return { name: input.name, pattern: input.getAttribute("pattern"), mismatch };
  }), Object.keys(EXPECTED_MISMATCH));
}

archiveTest("every slug field rejects invalid names through browser pattern validation", async ({ page, app }) => {
  const errors = collectPatternErrors(page);
  const covered = new Set();
  for (const path of [
    "/projects/complete",
    "/projects/restore",
    "/projects/complete/creator-sessions/sample/derive",
    "/projects/complete/creator-sessions/sample/reuse",
  ]) {
    await page.goto(`${app.origin}${path}`);
    const inputs = await slugInputs(page);
    expect(inputs.length, path).toBeGreaterThan(0);
    for (const { name, pattern, mismatch } of inputs) {
      expect(pattern, `${path} ${name}`).toBe(SLUG_PATTERN);
      expect(mismatch, `${path} ${name}`).toEqual(EXPECTED_MISMATCH);
      covered.add(`${path.includes("/derive") ? "derive" : path.includes("/reuse") ? "reuse" : "project"}:${name}`);
    }
  }
  expect([...covered].sort()).toEqual([
    "derive:session",
    "project:archive_name",
    "project:confirm_project_key",
    "project:confirm_target_project_key",
    "project:session",
    "project:session_name",
    "reuse:session",
  ]);

  // Re-review has no JavaScript slug check, so the browser must stop the
  // request before it reaches the server.
  const posts = [];
  page.on("request", (request) => {
    if (request.method() === "POST") posts.push(request.url());
  });
  const session = page.locator('[name="session"]');
  await expect(session).toBeVisible();
  await session.fill("Bad-Name");
  await page.locator("[data-synapse-submit]").click();
  expect(await session.evaluate((input) => input.matches(":invalid") && input.validationMessage.length > 0)).toBe(true);
  await expect(page).toHaveURL(`${app.origin}/projects/complete/creator-sessions/sample/reuse`);
  expect(posts).toEqual([]);
  expect(errors).toEqual([]);
});

inboxTest("the inbox session field rejects invalid names through browser pattern validation", async ({ page, app }) => {
  const errors = collectPatternErrors(page);
  await page.goto(`${app.origin}/projects/pending`);
  const inbox = (await slugInputs(page)).filter(({ name }) => name === "inbox_session");
  expect(inbox).toEqual([{ name: "inbox_session", pattern: SLUG_PATTERN, mismatch: EXPECTED_MISMATCH }]);
  expect(errors).toEqual([]);
});
