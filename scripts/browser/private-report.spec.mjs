import { readFile } from "node:fs/promises";
import { isolatedTest as test, expect, original, current, output } from "./fixtures.mjs";

const routeFor = (origin) => `${origin}/api/v1/projects/complete/creator-sessions/sample`;

test("a complete session saves its freshly checked private JSON without external requests", async ({ page, app }) => {
  await page.goto(`${app.origin}/projects/complete/creator-sessions/sample?lang=en`);
  await page.evaluate(() => {
    const create = URL.createObjectURL.bind(URL);
    const revoke = URL.revokeObjectURL.bind(URL);
    const click = HTMLAnchorElement.prototype.click;
    window.__privateReportUrl = null;
    window.__privateReportRevoked = [];
    URL.createObjectURL = blob => {
      const url = create(blob);
      if (blob.type.startsWith("application/json")) window.__privateReportUrl = url;
      return url;
    };
    URL.revokeObjectURL = url => { window.__privateReportRevoked.push(url); revoke(url); };
    HTMLAnchorElement.prototype.click = function () {
      click.call(this);
      if (this.href === window.__privateReportUrl) {
        // Hide the page in the same task as the download, before URL timers.
        window.dispatchEvent(new Event("pagehide"));
      }
    };
  });
  await expect(page.getByRole("heading", { name: "Save this record", exact: true })).toBeVisible();
  const button = page.getByRole("button", { name: "Save private record (JSON)", exact: true });
  const external = [];
  page.on("request", request => { if (new URL(request.url()).origin !== app.origin) external.push(request.url()); });
  const requested = page.waitForRequest(request => request.url() === routeFor(app.origin));
  const downloaded = page.waitForEvent("download");
  await button.focus();
  await page.keyboard.press("Enter");
  const [request, download] = await Promise.all([requested, downloaded]);
  expect(request.method()).toBe("GET");
  expect(await request.headerValue("x-synapse-local-token")).toBe(await page.locator('meta[name="synapse-local-token"]').getAttribute("content"));
  expect(download.suggestedFilename()).toBe("sample-private-report.json");
  const detail = JSON.parse(await readFile(await download.path(), "utf8"));
  expect(detail.state).toBe("complete");
  expect(detail.report.session).toBe("sample");
  expect(detail.report.proposal_head).toBe(await page.locator("[data-private-report]").getAttribute("data-proposal-head"));
  expect(detail.report.decision_head).toBe(await page.locator("[data-private-report]").getAttribute("data-decision-head"));
  expect(detail.report.rationale).toBe("Review fixture");
  expect(typeof detail.report.project_id).toBe("string");
  expect(typeof detail.report.creator_id).toBe("string");
  await expect(page.locator("[data-private-report-status]")).toHaveText("Private record download started.");
  expect(await page.evaluate(() => window.__privateReportUrl !== null
    && window.__privateReportRevoked.includes(window.__privateReportUrl))).toBe(true);
  expect(external).toEqual([]);
});

test("the saved JSON retains a private generation note, pin, and rationale", async ({ page, app }) => {
  const session = "private-report-details";
  const note = "PRIVATE_NOTE_CANARY";
  const pin = "PRIVATE_PIN_CANARY";
  const rationale = "PRIVATE_RATIONALE_CANARY";
  await page.goto(`${app.origin}/projects/reviews`);
  await page.locator('[name="session"]').fill(session);
  await page.locator('[name="creator_name"]').fill("Private report reviewer");
  await page.locator('[name="subject_label"]').fill("Private report fixture");
  for (const [name, file] of [["original_image", original], ["current_image", current], ["ai_output", output]]) {
    await page.locator(`[name="${name}"]`).setInputFiles(file);
  }
  await page.locator("details summary").click();
  await page.locator('[name="generation_prompt"]').fill(note);
  await page.getByRole("button", { name: "Proposalを作成", exact: true }).click();
  await page.waitForURL(`**/creator-sessions/${session}`);
  await page.getByRole("button", { name: "中央にピンを追加", exact: true }).click();
  await page.getByLabel("ピン 1 のメモ", { exact: true }).fill(pin);
  await page.getByLabel("Rationale（任意）", { exact: true }).fill(rationale);
  page.once("dialog", dialog => dialog.accept());
  const navigation = page.waitForEvent("framenavigated", { predicate: frame => frame === page.mainFrame() });
  await page.getByRole("button", { name: "Defer", exact: true }).click();
  await navigation;
  const downloaded = page.waitForEvent("download");
  await page.getByRole("button", { name: "非公開の記録を保存（JSON）", exact: true }).click();
  const detail = JSON.parse(await readFile(await (await downloaded).path(), "utf8"));
  expect(detail.report.rationale).toBe(rationale);
  expect(detail.report.generation_note.prompt).toBe(note);
  expect(detail.report.annotations.pins[0].note).toBe(pin);
});

test("a malformed, non-complete, or unavailable response shows an error and starts no download", async ({ page, app }) => {
  await page.goto(`${app.origin}/projects/complete/creator-sessions/sample`);
  const button = page.getByRole("button", { name: "非公開の記録を保存（JSON）", exact: true });
  const status = page.locator("[data-private-report-status]");
  let downloads = 0;
  page.on("download", () => { downloads += 1; });
  const endpoint = routeFor(app.origin);
  const mismatched = JSON.stringify({ state: "complete", report: { session: "sample", proposal_head: "wrong", decision_head: "wrong" } });
  for (const body of ["{", JSON.stringify({ state: "pending_review" }), mismatched]) {
    await page.route(endpoint, route => route.fulfill({ status: 200, contentType: "application/json", body }));
    await button.click();
    await expect(status).toContainText("記録を確認できませんでした");
    await expect(button).toBeEnabled();
    await page.unroute(endpoint);
  }
  await page.route(endpoint, route => route.abort("failed"));
  await button.click();
  await expect(status).toContainText("記録を確認できませんでした");
  await page.unroute(endpoint);
  expect(downloads).toBe(0);
});

test("pending sessions do not offer a private report save action", async ({ page, app }) => {
  await page.goto(`${app.origin}/projects/reviews`);
  await page.locator('[name="session"]').fill("private-report-pending");
  await page.locator('[name="creator_name"]').fill("Private report reviewer");
  await page.locator('[name="subject_label"]').fill("Pending fixture");
  for (const [name, file] of [["original_image", original], ["current_image", current], ["ai_output", output]]) {
    await page.locator(`[name="${name}"]`).setInputFiles(file);
  }
  await page.getByRole("button", { name: "Proposalを作成", exact: true }).click();
  await page.waitForURL("**/creator-sessions/private-report-pending");
  await expect(page.locator("[data-private-report]")).toHaveCount(0);
  await app.restart();
  await page.goto(`${app.origin}/projects/reviews/creator-sessions/private-report-pending`);
  await expect(page.locator("[data-private-report]")).toHaveCount(0);
});

test("the save control needs JavaScript and a double click reads only once", async ({ page, app, browser }) => {
  const context = await browser.newContext({ javaScriptEnabled: false });
  try {
    const reader = await context.newPage();
    await reader.goto(`${app.origin}/projects/complete/creator-sessions/sample`);
    await expect(reader.getByRole("button", { name: "非公開の記録を保存（JSON）", exact: true })).toBeHidden();
    await expect(reader.getByText("記録を保存するにはJavaScriptを有効にしてください。", { exact: true })).toBeVisible();
  } finally {
    await context.close();
  }

  await page.goto(`${app.origin}/projects/complete/creator-sessions/sample`);
  const endpoint = routeFor(app.origin);
  let reads = 0;
  await page.route(endpoint, async route => {
    reads += 1;
    await new Promise(resolve => setTimeout(resolve, 100));
    await route.continue();
  });
  const button = page.getByRole("button", { name: "非公開の記録を保存（JSON）", exact: true });
  const downloaded = page.waitForEvent("download");
  await button.dblclick();
  await downloaded;
  expect(reads).toBe(1);
  await page.unroute(endpoint);
});
