# Browser regression tests

Run the commands in [CONTRIBUTING.md](../../CONTRIBUTING.md#browser-regression-tests)
from the repository root. The localhost scenarios use built debug binaries,
temporary repositories, committed synthetic mural fixtures, and an ephemeral
`127.0.0.1` server. The publication scenarios load frozen public bundles
directly with `file://`; they do not start a server or contact a network. No
user repository or hosted service is needed.

Scenarios cover completed and pending sessions, keyboard zoom and
scrolling, native modal focus and Escape, responsive layout, automated axe
checks, attachment-only and broken media, image cleanup, and JavaScript-off
history. Import preflight coverage includes no-upload local previews, byte
limits, file clearing, opaque/corrupt input compatibility, pending-request
locking, stale asynchronous results, reset/lifecycle cleanup, and mobile
accessibility. Decision review coverage includes all three outcomes, rationale
round trips and markup escaping, cancel without writes, multibyte limits,
failed-request preservation, no-JavaScript summaries, and narrow-screen axe
checks. Browser lifecycle events are dispatched explicitly in the cleanup
regression; this does not assert cross-browser back/forward-cache eligibility.
Automated accessibility checks do not establish full accessibility conformance.
The publication checks cover the two frozen bundles with Chromium axe
critical/serious findings, keyboard `<details>` operation, heading and main
landmark structure, link labels and local resolution, and reflow at 320 CSS
pixels plus a 200% zoom equivalent. Screen-reader review remains a manual
publication gate. See [the publication gate notes](../../docs/publication_accessibility_gate.md)
for the automated scope and approved exception for the frozen `incomplete-only`
bundle's disclosure-token overflow. The two reflow checks run normally and
report the exact known defect; other failures and a resolved defect still fail.
Copied and future bundles receive no exception.

To test a future bundle directory (containing `index.html`) or a paired root
with `complete/` and `incomplete-only/`, pass it explicitly. The full suite
uses the frozen corpus by default.

```bash
npm --prefix scripts/browser run test:publication -- --bundle-root /absolute/path/to/bundles
```

`SYNAPSEGIT_PUBLICATION_BUNDLE_ROOT` supplies the same root when invoking
Playwright directly. The tests only load each `index.html`; they do not crawl
the bundle's links.

`package-lock.json` pins test-only dependencies. Playwright packages use
Apache-2.0; `@axe-core/playwright` and `axe-core` use MPL-2.0. Their installed
packages contain their license notices. These development tools are neither
embedded in the application assets nor distributed in the Rust release
binaries; the root generated third-party notice inventory covers Cargo
components.
