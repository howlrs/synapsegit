import AxeBuilder from "@axe-core/playwright";
import { test, expect, original, current, output } from "./fixtures.mjs";

async function begin(page, app, session) {
  await page.goto(`${app.origin}/projects/reviews`);
  await page.locator('[name="session"]').fill(session);
  await page.locator('[name="creator_name"]').fill("Browser reviewer");
  await page.locator('[name="subject_label"]').fill("Review summary fixture");
  await page.locator('[name="original_image"]').setInputFiles(original);
  await page.locator('[name="current_image"]').setInputFiles(current);
  await page.locator('[name="ai_output"]').setInputFiles(output);
  await page.getByRole("button", { name: "Proposalを作成" }).click();
  await page.waitForURL(`**/creator-sessions/${session}`);
}

const rationale = (page) => page.getByLabel("Rationale（任意）", { exact: true });
const recorded = (page) => page.locator("[data-decision-rationale]");

async function recordAndReload(page, action) {
  // A decision performs an async fetch before reloading this same URL. Wait for
  // that navigation explicitly; a locator timeout should not time the fsck/report.
  const navigation = page.waitForEvent("framenavigated", { predicate: (frame) => frame === page.mainFrame() });
  await action();
  await navigation;
  await page.waitForLoadState("domcontentloaded");
}

for (const [button, disposition, outcome] of [
  ["Adopt", "adopt", "AI outputを変更せず採用"],
  ["Reject", "reject", "AI outputを採用しない"],
  ["Defer", "defer", "AI outputの採用を保留"],
]) {
  test(`${button}: explicit outcome and recorded rationale survive a fresh page load`, async ({ page, app }) => {
    const session = `review-${disposition}`;
    await begin(page, app, session);
    const choice = page.getByRole("button", { name: button, exact: true });
    await expect(choice).toHaveAccessibleDescription(new RegExp(outcome));
    const reason = `一行目: ${button}\n<script>window.reviewInjected = true</script>\n${"長".repeat(80)} & <理由>`;
    await rationale(page).fill(reason);
    let prompt;
    page.once("dialog", async (dialog) => { prompt = dialog.message(); await dialog.accept(); });
    const requestPromise = page.waitForRequest((request) => request.method() === "POST" && request.url().endsWith("/decisions"));
    await recordAndReload(page, () => choice.click());
    const request = await requestPromise;
    expect(request.postDataJSON()).toMatchObject({ disposition, rationale: reason });
    expect(prompt).toContain(session);
    expect(prompt).toContain("reviews");
    expect(prompt).toContain(outcome);
    await expect(page.getByRole("heading", { name: "記録した判断", exact: true })).toBeVisible();
    await expect(recorded(page)).toHaveText(reason);
    await expect(recorded(page).locator("script")).toHaveCount(0);
    await expect(page.locator('[data-decision-outcome]')).toContainText(outcome);
    await expect(page.getByRole("button", { name: button, exact: true })).toHaveCount(0);
    expect(await page.evaluate(() => window.reviewInjected)).toBeUndefined();
    await page.reload();
    await expect(recorded(page)).toHaveText(reason);
  });
}

test("cancel keeps rationale and records no decision; keyboard retry remains explicit", async ({ page, app }) => {
  await begin(page, app, "review-cancel");
  let posts = 0;
  page.on("request", (request) => { if (request.method() === "POST") posts += 1; });
  await rationale(page).fill("比較をやり直すため戻ります。");
  const choice = page.getByRole("button", { name: "Defer", exact: true });
  page.once("dialog", (dialog) => dialog.dismiss());
  await choice.focus();
  await page.keyboard.press("Enter");
  await expect(page.locator("[data-synapse-status]")).toHaveText("Decisionは送信されませんでした。");
  expect(posts).toBe(0);
  await expect(rationale(page)).toHaveValue("比較をやり直すため戻ります。");
  await expect(choice).toBeFocused();
  await expect(page.getByText("Deferも含め、記録後にこのセッションの判断を変更・再開する機能はありません。", { exact: true })).toBeVisible();
  page.once("dialog", (dialog) => dialog.accept());
  await recordAndReload(page, () => page.keyboard.press("Enter"));
  await expect(recorded(page)).toHaveText("比較をやり直すため戻ります。");
  expect(posts).toBe(1);
});

test("UTF-8 limit blocks before confirmation and failed submission preserves the exact rationale", async ({ page, app }) => {
  await begin(page, app, "review-limit");
  let confirms = 0;
  let posts = 0;
  page.on("dialog", async (dialog) => { confirms += 1; await dialog.accept(); });
  const counter = page.locator("[data-decision-text-count]");
  await expect(counter).toHaveText("0 / 5000 bytes");
  await rationale(page).fill("あ".repeat(1667));
  await expect(counter).toContainText("5001 / 5000 bytes");
  await expect(rationale(page)).toHaveAttribute("aria-invalid", "true");
  await page.getByRole("button", { name: "Adopt", exact: true }).click();
  expect(confirms).toBe(0);
  let release;
  const gate = new Promise((resolve) => { release = resolve; });
  await page.route("**/creator-sessions/review-limit/decisions", async (route) => {
    posts += 1;
    expect(route.request().postDataJSON().rationale).toBe("あ".repeat(1666) + "ab");
    await gate;
    await route.fulfill({ status: 503, contentType: "application/problem+json", body: JSON.stringify({ title: "Try again later", detail: "Temporary review failure", code: "internal_error" }) });
  });
  await rationale(page).fill("あ".repeat(1666) + "ab");
  await expect(counter).toHaveText("5000 / 5000 bytes");
  await expect(rationale(page)).toHaveAttribute("aria-invalid", "false");
  await page.getByRole("button", { name: "Adopt", exact: true }).click();
  try {
    await expect(rationale(page)).toBeDisabled();
    for (const name of ["Adopt", "Reject", "Defer"]) await expect(page.getByRole("button", { name, exact: true })).toBeDisabled();
  } finally { release(); }
  await expect(page.locator("[data-synapse-status]")).toHaveText("Temporary review failure");
  await expect(rationale(page)).toBeEnabled();
  await expect(rationale(page)).toHaveValue("あ".repeat(1666) + "ab");
  expect(confirms).toBe(1);
  expect(posts).toBe(1);
  await page.unroute("**/creator-sessions/review-limit/decisions");
  await recordAndReload(page, () => page.getByRole("button", { name: "Adopt", exact: true }).click());
  await expect(recorded(page)).toHaveText("あ".repeat(1666) + "ab");
  expect(confirms).toBe(2);
});

test("empty rationale has an explicit completed state and completed summary works without JavaScript", async ({ page, app, browser }) => {
  await begin(page, app, "review-empty");
  page.once("dialog", (dialog) => dialog.accept());
  await recordAndReload(page, () => page.getByRole("button", { name: "Reject", exact: true }).click());
  await expect(recorded(page)).toHaveText("理由は記録されていません。");
  const context = await browser.newContext({ javaScriptEnabled: false });
  try {
    const reader = await context.newPage();
    await reader.goto(`${app.origin}/projects/reviews/creator-sessions/review-empty`);
    await expect(reader.getByRole("heading", { name: "記録した判断", exact: true })).toBeVisible();
    await expect(recorded(reader)).toHaveText("理由は記録されていません。");
    await expect(reader.locator('[data-decision-outcome]')).toContainText("AI outputを採用しない");
  } finally { await context.close(); }
});

test("review choices and long recorded rationale remain accessible on a narrow screen", async ({ page, app }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await begin(page, app, "review-mobile");
  await expect(page.getByRole("button", { name: "Adopt", exact: true })).toBeVisible();
  expect((await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa", "wcag21aa"]).analyze()).violations).toEqual([]);
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
  await rationale(page).fill("LongWord".repeat(200) + "\n記録した理由");
  page.once("dialog", (dialog) => dialog.accept());
  await recordAndReload(page, () => page.getByRole("button", { name: "Adopt", exact: true }).click());
  await expect(recorded(page)).toContainText("記録した理由");
  expect(await recorded(page).evaluate((element) => getComputedStyle(element).whiteSpace)).toBe("pre-wrap");
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
  expect((await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa", "wcag21aa"]).analyze()).violations).toEqual([]);
});
