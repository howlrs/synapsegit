# Local browser regression tests

Run the commands in [CONTRIBUTING.md](../../CONTRIBUTING.md#browser-regression-tests)
from the repository root. The suite uses built debug binaries, temporary
repositories, the committed synthetic mural fixtures, and an ephemeral
`127.0.0.1` server. No user repository or hosted service is needed.

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

`package-lock.json` pins test-only dependencies. Playwright packages use
Apache-2.0; `@axe-core/playwright` and `axe-core` use MPL-2.0. Their installed
packages contain their license notices. These development tools are neither
embedded in the application assets nor distributed in the Rust release
binaries; the root generated third-party notice inventory covers Cargo
components.
