import { inboxTest as test, expect, original, current, output } from "./fixtures.mjs";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";

// Follows docs/ai_agent_guide.md: the agent places a candidate with the CLI,
// the person (simulated by the browser) reviews and decides, and the agent
// reads the result with the CLI.  No CLI step records the decision.
test("the AI agent guide flow keeps the decision with the person", async ({ page, app }) => {
  test.setTimeout(120_000);
  const work = await mkdtemp(path.join(tmpdir(), "synapse-agent-guide-"));
  try {
    const note = path.join(work, "note.json");
    await writeFile(note, JSON.stringify({ tool: "image generator", model: "model-a", prompt: "Restore the faded sky", intent: "Candidate for the north wall" }));

    // Steps 3-4: the agent writes the note and places the candidate.
    const put = JSON.parse(app.cli(
      "inbox", "put", app.inboxRoot, "guide-smoke", original, current, output,
      "--subject", "North wall mural", "--creator", "Aki",
      "--generation-note-file", note, "--format", "json",
    ));
    expect(put.format).toBe("synapsegit-cli-inbox-put-v1");
    expect(put.decision_recorded).toBe(false);
    const before = JSON.parse(app.cli("creator-list", app.projectPath("pending"), "--format", "json"));
    expect(before.sessions).toEqual([]);

    // Step 5: the person reviews the candidate and decides in the browser.
    await page.goto(`${app.origin}/projects/pending`);
    await expect(page.locator("[data-import-inbox]")).toBeVisible();
    await page.getByRole("button", { name: "確認する" }).click();
    const preview = page.locator("form[data-import-inbox-preview]");
    await expect(preview).toBeVisible();
    await expect(preview.getByLabel("Inboxのセッション名", { exact: true })).toHaveValue("inbox-guide-smoke");
    await preview.getByRole("button", { name: "Proposalを作成" }).click();
    await page.waitForURL("**/creator-sessions/inbox-guide-smoke");
    page.once("dialog", dialog => dialog.accept());
    const navigation = page.waitForEvent("framenavigated", { predicate: (frame) => frame === page.mainFrame() });
    await page.getByRole("button", { name: "保留", exact: true }).click();
    await navigation;
    await page.waitForLoadState("domcontentloaded");
    await expect(page.getByRole("heading", { name: "記録した判断", exact: true })).toBeVisible();

    // Step 6: the agent reads the overview and the verified record.
    const list = JSON.parse(app.cli("creator-list", app.projectPath("pending"), "--format", "json"));
    expect(list.verified).toBe(false);
    expect(list.sessions).toHaveLength(1);
    expect(list.sessions[0]).toMatchObject({ session: "inbox-guide-smoke", state: "complete", disposition: "defer", subject_label: "North wall mural", creator_name: "Aki" });
    const report = JSON.parse(app.cli("creator-report", app.projectPath("pending"), "inbox-guide-smoke", "--format", "json"));
    expect(report.format).toBe("synapsegit-cli-creator-report-v1");
    expect(report.disposition).toBe("defer");
    expect(report.subject_label).toBe("North wall mural");
    expect(report.generation_note).toMatchObject({ availability: "present", tool: "image generator", intent: "Candidate for the north wall" });
  } finally {
    await rm(work, { recursive: true, force: true });
  }
});
