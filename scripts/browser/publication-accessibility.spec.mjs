import AxeBuilder from "@axe-core/playwright";
import { test, expect } from "@playwright/test";
import { mkdtemp, readFile, writeFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { pathToFileURL } from "node:url";
import { bundleRoot, defaultBundleRoot, headingLevels, hasSkippedHeadingLevel, publicationBundles, publicationReflowWaiver } from "./publication-accessibility.mjs";

const bundles = await publicationBundles(bundleRoot());

test.beforeEach(async ({ context }) => {
  await context.route("**/*", route => {
    if (new URL(route.request().url()).protocol === "file:") return route.continue();
    return route.abort();
  });
});

async function openBundle(page, bundle) {
  await page.goto(bundle.url);
  await expect(page.locator("main")).toBeVisible();
}

async function visibleTextNodes(page) {
  return page.evaluate(() => {
    const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
    const visible = [];
    let index = 0;
    while (walker.nextNode()) {
      const node = walker.currentNode;
      const style = getComputedStyle(node.parentElement);
      const range = document.createRange();
      range.selectNodeContents(node);
      if (node.textContent.trim() && !node.parentElement.closest("script, style") &&
          style.visibility !== "hidden" && style.visibility !== "collapse" &&
          [...range.getClientRects()].some(rect => rect.width > 0 && rect.height > 0)) {
        visible.push(index);
      }
      index += 1;
    }
    return visible;
  });
}

async function reflowBaseline(page, bundle) {
  await openBundle(page, bundle);
  // Include disclosure content in reflow checks, not only its closed summary.
  await page.locator("details").evaluateAll(elements => elements.forEach(element => { element.open = true; }));
  return visibleTextNodes(page);
}

async function assertNoHorizontalOverflow(page, baseline, waiver) {
  expect(await visibleTextNodes(page), "content visible at normal width must remain visible").toEqual(baseline);
  const dimensions = await page.evaluate(tokenText => {
    const tokens = [...document.querySelectorAll("main > section > ul > li > strong")]
      .filter(element => element.textContent === tokenText &&
        element.closest("section").querySelector("h2")?.textContent === "Disclosure and limits");
    const token = tokens.length === 1 ? tokens[0] : undefined;
    const beyondViewport = rect => rect.right > document.documentElement.clientWidth + 1 || rect.left < -1;
    // Text can overflow a parent whose own border box still fits the viewport.
    const textOverflow = [];
    const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
    while (walker.nextNode()) {
      const node = walker.currentNode;
      if (!node.textContent.trim() || node.parentElement.closest("script, style")) continue;
      const range = document.createRange();
      range.selectNodeContents(node);
      if ([...range.getClientRects()].some(beyondViewport)) {
        textOverflow.push({ text: node.textContent.trim(), knownToken: node.parentElement === token });
      }
    }
    return {
    clientWidth: document.documentElement.clientWidth,
    scrollWidth: document.documentElement.scrollWidth,
    zoom: Number(getComputedStyle(document.documentElement).zoom),
    tokenCount: tokens.length,
    tokenRight: token?.getBoundingClientRect().right,
    textOverflow,
    clipped: [...document.body.querySelectorAll("*")].map(element => {
      const rect = element.getBoundingClientRect();
      return { tag: element.tagName, text: element.textContent.trim().slice(0, 80), left: rect.left, right: rect.right, knownToken: element === token };
    }).filter(beyondViewport),
    internallyClipped: [...document.body.querySelectorAll("*")].filter(element => {
      const style = getComputedStyle(element);
      const clips = value => ["hidden", "clip", "auto", "scroll"].includes(value);
      return (element.clientWidth > 0 && element.scrollWidth > element.clientWidth + 1 && clips(style.overflowX)) ||
        (element.clientHeight > 0 && element.scrollHeight > element.clientHeight + 1 && clips(style.overflowY));
    }).map(element => ({ tag: element.tagName, text: element.textContent.trim().slice(0, 80) })),
    };
  }, waiver?.token);
  expect(dimensions.internallyClipped, "element-local clipping must never be waived").toEqual([]);
  if (!waiver) {
    expect(dimensions.scrollWidth).toBeLessThanOrEqual(dimensions.clientWidth);
    expect(dimensions.clipped).toEqual([]);
    expect(dimensions.textOverflow).toEqual([]);
    return;
  }

  expect([[320, 1], [640, 2]]).toContainEqual([dimensions.clientWidth, dimensions.zoom]);
  expect(dimensions.tokenCount).toBe(1);
  expect(dimensions.scrollWidth, "known overflow disappeared; review or remove the waiver").toBeGreaterThan(dimensions.clientWidth);
  expect(dimensions.clipped.map(element => element.knownToken), "only the known disclosure token may overflow").toEqual([true]);
  expect(dimensions.textOverflow, "no other overflowing text may be covered by the waiver").toEqual([
    { text: waiver.token, knownToken: true },
  ]);
  expect(Math.abs(dimensions.scrollWidth - dimensions.tokenRight), "document overflow must end at the known token").toBeLessThanOrEqual(1);
  return { id: waiver.id, sha256: waiver.sha256, viewport: dimensions.clientWidth, zoom: dimensions.zoom, scrollWidth: dimensions.scrollWidth };
}

function reportWaiver(finding) {
  if (!finding) return;
  test.info().annotations.push({ type: "known-reflow-defect", description: JSON.stringify(finding) });
  console.log("publication_reflow_waiver: " + JSON.stringify(finding));
}

for (const bundle of bundles) {
  test(`${bundle.name} publication bundle has no serious or critical axe findings`, async ({ page }) => {
    await openBundle(page, bundle);
    const results = await new AxeBuilder({ page }).analyze();
    expect(results.violations.filter(({ impact }) => impact === "critical" || impact === "serious")).toEqual([]);
  });

  test(`${bundle.name} publication bundle preserves headings, landmarks, and link purpose`, async ({ page }) => {
    await openBundle(page, bundle);
    const structure = await page.evaluate(() => ({
      headings: [...document.querySelectorAll("h1,h2,h3,h4,h5,h6")].map(heading => ({ tagName: heading.tagName })),
      mainCount: document.querySelectorAll("main, [role=main]").length,
      bodyMainCount: document.querySelectorAll("body > main").length,
      nestedMainCount: document.querySelectorAll("main main, [role=main] main, main [role=main]").length,
      links: [...document.querySelectorAll("a[href]")].map(link => ({ name: link.textContent.trim() || link.getAttribute("aria-label") || "", href: link.getAttribute("href"), resolved: link.href })),
    }));
    const levels = headingLevels(structure.headings);
    expect(levels[0]).toBe(1);
    expect(hasSkippedHeadingLevel(levels)).toBe(false);
    expect(structure.mainCount).toBe(1);
    expect(structure.bodyMainCount).toBe(1);
    expect(structure.nestedMainCount).toBe(0);
    for (const link of structure.links) {
      expect(link.name).not.toBe("");
      expect(link.href).not.toBe("");
      expect(link.resolved.startsWith("file:")).toBe(true);
    }
  });

  test(`${bundle.name} publication bundle checks reflow at 320 CSS pixels`, async ({ page }) => {
    const waiver = await publicationReflowWaiver(bundle);
    const baseline = await reflowBaseline(page, bundle);
    await page.setViewportSize({ width: 320, height: 720 });
    reportWaiver(await assertNoHorizontalOverflow(page, baseline, waiver));
  });

  test(`${bundle.name} publication bundle checks reflow at 200% zoom equivalent`, async ({ page }) => {
    const waiver = await publicationReflowWaiver(bundle);
    const baseline = await reflowBaseline(page, bundle);
    await page.setViewportSize({ width: 640, height: 720 });
    await page.evaluate(() => { document.documentElement.style.zoom = "2"; });
    reportWaiver(await assertNoHorizontalOverflow(page, baseline, waiver));
  });
}

for (const bundle of bundles) {
  test(`${bundle.name} publication bundle supports keyboard traversal`, async ({ page }) => {
    await openBundle(page, bundle);
    const focusableCount = await page.locator("summary, a[href]").count();
    expect(focusableCount).toBeGreaterThan(0);
    await page.locator("body").click({ position: { x: 1, y: 1 } });
    const focusedTargets = [];
    for (let index = 0; index < focusableCount; index += 1) {
      await page.keyboard.press("Tab");
      focusedTargets.push(await page.evaluate(() => [...document.querySelectorAll("summary, a[href]")].indexOf(document.activeElement)));
    }
    expect(focusedTargets).toEqual([...Array(focusableCount).keys()]);
  });
}

for (const bundle of bundles) {
  test(`${bundle.name} publication bundle toggles each available details element from the keyboard`, async ({ page }) => {
    await openBundle(page, bundle);
    const detailsCount = await page.locator("details").count();
    if (bundle.name === "complete") expect(detailsCount).toBeGreaterThan(0);
    if (detailsCount === 0) return;

    await page.locator("body").click({ position: { x: 1, y: 1 } });
    for (let index = 0; index < detailsCount; index += 1) {
      let summaryReached = await page.evaluate(() => document.activeElement?.tagName === "SUMMARY");
      for (let attempt = 0; !summaryReached && attempt < detailsCount + 20; attempt += 1) {
        await page.keyboard.press("Tab");
        summaryReached = await page.evaluate(() => document.activeElement?.tagName === "SUMMARY");
      }
      expect(summaryReached).toBe(true);
      const details = page.locator("details").filter({ has: page.locator("summary:focus") }).first();
      await page.keyboard.press("Space");
      await expect(details).toHaveAttribute("open", "");
      await page.keyboard.press("Space");
      await expect(details).not.toHaveAttribute("open", "");
      if (index + 1 < detailsCount) await page.keyboard.press("Tab");
    }
  });
}

test("axe check fails for a deliberately broken publication fixture", async ({ page }) => {
  const directory = await mkdtemp(path.join(tmpdir(), "synapse-publication-a11y-"));
  try {
    const fixture = path.join(directory, "broken.html");
    await writeFile(fixture, "<!doctype html><html><body><main><h1>Broken fixture</h1><img src=\"missing.png\"><button></button></main></body></html>");
    await page.goto(pathToFileURL(fixture).href);
    const results = await new AxeBuilder({ page }).analyze();
    expect(results.violations.some(({ impact }) => impact === "critical" || impact === "serious")).toBe(true);
  } finally { await rm(directory, { recursive: true, force: true }); }
});

for (const [name, rule] of [
  ["hidden responsive content", "display:none"],
  ["clipped responsive content", "width:40px;height:10px;overflow:hidden"],
]) {
  test(`reflow check rejects ${name}`, async ({ page }) => {
    await page.setContent(`<!doctype html><html lang="en"><head><style>@media(max-width:640px){p{${rule}}}</style></head><body><main><h1>Reflow control</h1><p>Content that must stay readable at narrow widths.</p></main></body></html>`);
    const baseline = await visibleTextNodes(page);
    await assertNoHorizontalOverflow(page, baseline);
    await page.setViewportSize({ width: 320, height: 720 });
    await expect(assertNoHorizontalOverflow(page, baseline)).rejects.toThrow();
  });
}

// Mutate only the browser DOM, never the golden artifact. Each condition must
// still fail while the approved token's overflow is present (or has vanished).
const frozenIncomplete = (await publicationBundles(defaultBundleRoot)).find(bundle => bundle.name === "incomplete-only");
for (const [width, zoom] of [[320, 1], [640, 2]]) {
  for (const scenario of ["other overflow", "hidden text", "internal clipping", "resolved defect"]) {
    test(`known reflow waiver rejects ${scenario} at ${width}px / ${zoom}x`, async ({ page }) => {
      const waiver = await publicationReflowWaiver(frozenIncomplete);
      const baseline = await reflowBaseline(page, frozenIncomplete);
      await page.setViewportSize({ width, height: 720 });
      const extent = await page.evaluate(({ zoom, scenario, token }) => {
        document.documentElement.style.zoom = String(zoom);
        const before = document.documentElement.scrollWidth;
        if (scenario === "other overflow") {
          // Leave the known token as the widest overflow. A second text node
          // spills without enlarging its parent box or document scrollWidth.
          const paragraph = document.querySelector("p.lead");
          paragraph.style.width = "210px";
          paragraph.style.marginLeft = "41px";
          paragraph.style.whiteSpace = "nowrap";
          paragraph.style.fontSize = "12px";
        } else if (scenario === "hidden text") {
          document.querySelector("p.lead").style.display = "none";
        } else if (scenario === "internal clipping") {
          const paragraph = document.querySelector("p.lead");
          paragraph.style.height = "1px";
          paragraph.style.overflow = "hidden";
        } else {
          [...document.querySelectorAll("strong")].find(element => element.textContent === token).style.overflowWrap = "anywhere";
        }
        return { before, after: document.documentElement.scrollWidth };
      }, { zoom, scenario, token: waiver.token });
      if (scenario === "other overflow") expect(extent.after).toBe(extent.before);
      const expected = {
        "other overflow": /no other overflowing text/,
        "hidden text": /content visible at normal width/,
        "internal clipping": /element-local clipping must never be waived/,
        "resolved defect": /known overflow disappeared/,
      }[scenario];
      await expect(assertNoHorizontalOverflow(page, baseline, waiver)).rejects.toThrow(expected);
    });
  }
}

test("the known reflow waiver does not apply to a copied or future bundle", async ({ page }) => {
  const directory = await mkdtemp(path.join(tmpdir(), "synapse-unwaived-reflow-"));
  try {
    const index = path.join(directory, "index.html");
    await writeFile(index, await readFile(frozenIncomplete.index));
    const bundle = { name: "incomplete-only", index, url: pathToFileURL(index).href };
    const waiver = await publicationReflowWaiver(bundle);
    expect(waiver).toBeUndefined();
    const baseline = await reflowBaseline(page, bundle);
    await page.setViewportSize({ width: 320, height: 720 });
    await expect(assertNoHorizontalOverflow(page, baseline, waiver)).rejects.toThrow();
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});
