# Genuine UI captures

The PNGs render the actual Neon Create/Resource module in the Console with its
real API client, SDK, styles, session and i18n provider. Only the API transport and
workspace/provider data are synthetic. A visible banner identifies that data.
The session has customer role `user`; write/reveal buttons are disabled for the
capture. No real customer, key, password or revealed URI appears.

- `neon-connect.png`: selected existing project, branch, database, role and endpoint.
- `neon-database.png`: safe settings, branches, hidden URI and local detach explanation.
- `neon-database-mobile-de.png`: the German mobile resource view.

The EU-built fixture is `apps/console/tests/neon-plugin-fixture.html` in
[spitzli/cloud](https://github.com/spitzli/cloud). Its fixture guide documents the
explicit build, loopback server and complete customer browser acceptance test.
After serving that built artifact, run the host's `neon-plugin-capture.mjs` with
`NEON_PLUGIN_FIXTURE_URL` and `NEON_PLUGIN_SCREENSHOTS_DIR` explicitly set. It
blocks external requests, checks customer role, overflow and hidden credentials,
disables writes, saves unedited PNGs and writes `provenance.json` with their hashes
and actual browser version. `PLAYWRIGHT_CHROMIUM_EXECUTABLE` can select an already
installed compatible Chromium executable. Never capture real provider credentials.

Captions mean **actual Console UI; synthetic fixture data; disabled write actions**.
They do not prove a live Neon account, healthy application or production rollout.
