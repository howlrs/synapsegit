# Publication browser accessibility gate

`scripts/browser/publication-accessibility.spec.mjs` opens a public bundle's
`index.html` in Chromium with `file://`. It accepts either the frozen paired
corpus root or one future bundle directory through
`SYNAPSEGIT_PUBLICATION_BUNDLE_ROOT` / `test:publication -- --bundle-root`.
The page context blocks service workers and aborts every request whose protocol
is not `file:`. It does not follow or fetch links.

The automated gate checks Chromium axe critical and serious findings, heading
level progression, one top-level main landmark, non-empty link text with a
locally resolved `href`, keyboard traversal, every available native details
element, and horizontal overflow or clipped boxes at 320 CSS pixels and a CSS
200% zoom equivalent. Reflow checks open disclosures, preserve the text nodes
visible at normal width, and reject element-local clipped or scrolling content.
Synthetic responsive hiding and clipping cases verify those checks fail when
content is lost. The deliberately broken axe fixture must produce a
critical or serious axe result, so a disabled or ineffective axe check cannot
pass silently.

The gate does not establish WCAG conformance or replace the protocol's manual
screen-reader review. It also does not repeat the frozen corpus's static checks
for external resources, active scripts, or forms, and it never changes or
regenerates corpus artifacts.

## Current frozen-corpus finding

As measured in Chromium on 2026-09-17, the frozen
`publication-comprehension/v1/bundles/incomplete-only/index.html` fails the
strict reflow checks. At a 320 CSS-pixel viewport its document `scrollWidth` is
386 while `clientWidth` is 320. At the test's 200% CSS zoom equivalent (a
640-pixel viewport with `documentElement.style.zoom = "2"`), `scrollWidth` is
773 while `clientWidth` is 640. In both cases the overflowing element is the
unbreakable `<strong>projection_fingerprint_unavailable</strong>` disclosure
token.

Reproduction and counterfactual check (Chromium 153.0.8010.12): each of the
two original reflow tests failed on all three repeated runs. In a temporary
copy, adding only `overflow-wrap:anywhere` to that one disclosure token made
the unchanged publication suite pass all 15 tests. The measured scroll widths
then became 320/320 and 640/640 respectively. Both frozen source HTML hashes
remained unchanged. This identifies one reproducible defect exercised by two
tests; the tests are capable of passing when that defect is absent.

The zoom probe is specifically CSS `zoom: 2` at a 640-pixel viewport, not an
automated browser-UI zoom test. The original also fits at a 1280-pixel viewport
with CSS `zoom: 2` (1280/1280). The result does not mean every 200% zoom view
fails, nor does it prove behavior in every browser or font environment.

## Approved exception and removal conditions

The user approved a narrow exception for this known defect. It is recorded as
`publication-v1-incomplete-disclosure-wrap` in the browser checks for
[issue #90](https://github.com/howlrs/synapsegit/issues/90). The v1 corpus remains
unchanged. Resolving the renderer defect still requires the separate
renderer/profile and corpus-version process described by the freeze policy.

The exception applies only to the repository's frozen `incomplete-only/index.html`
at SHA-256 `b64054b99407398ec75b7aeeb64993fb2e8e58dd0f529445a7c1d0d9f5187906`,
at 320 pixels / CSS zoom 1 and 640 pixels / CSS zoom 2. A copied or future
bundle is checked strictly, even if its name and bytes match. Changing the
frozen artifact fails the exception's identity check.

Both checks execute normally; neither is skipped or marked as an expected
failure. They require the known disclosure token to be the sole overflowing
element and text node, with the document's scroll extent ending at that token
(within one pixel of rounding). Recorded pixel measurements are evidence,
not a blanket allowance for everything within a 386- or 773-pixel boundary.
Normal-width text must remain visible and element-local clipping must be absent.
Any other failure still fails the test. If the known overflow disappears,
the check also fails with an instruction to review or remove the exception.

Successful exception checks emit `publication_reflow_waiver` with the exception
ID, artifact hash, viewport, zoom, and measured scroll width, and add a
`known-reflow-defect` test annotation. A passing CI result therefore records an
accepted exception, not a clean reflow result or completed accessibility review.
Browser controls verify that unrelated text overflow (including an overflow
smaller than the known token), hidden content, internal clipping, and resolution
of the defect are rejected at both sizes. Another control verifies that an
identical temporary copy receives no exception.
