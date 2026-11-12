# Standalone build

This repository is independent of the Core Rust workspace. Package/binary:
`spitzli-neon` 0.1.0. Native protocol/SDK/data protocol: v2. Both SDK crates use the
published Git revision `8ed57d89c3263b050f126e4c2ba7f6cdebb0daa6` from
`https://github.com/spitzli/cloud-plugin-sdk`. Rust is pinned to 1.98.1. Both
`Cargo.lock` files are included. No local path dependency or new SDK release is
required. `standalone.json` records the machine-readable source contract.

## Qualified native checks

Run on the existing qualified EU build server, inside full-process-sandbox
rootless BuildKit. Do not compile native or application code on the desktop.
The helper rejects root, noncanonical/untrusted sockets, mismatched pinned
buildctl and unqualified builders; it never uses a no-process-sandbox fallback.
With the existing qualified runner's private environment and external temporary
directory set:

```sh
python3 scripts/plugin-release/build_oci.py --check < /dev/null
```

The immutable Dockerfile `check` stage runs `cargo check --locked --all-targets`
and `cargo test --locked --all-targets`, with three jobs. For a full review also
run `cargo clippy --locked --all-targets -- -D warnings` on that EU runner and
`cargo test --locked --manifest-path tools/manifest-validator/Cargo.toml` there.
Locally, formatting and lockfile metadata inspection do not build the runtime:

```sh
cargo fmt --check
cargo metadata --locked --offline --no-deps --format-version 1
```

Focused native fixtures exercise the original host callback ABI, all action
boundaries, exact write input forwarding, safe provider projections, malformed
responses, no provider deletion, panel redaction and ephemeral URI handling.
These are fake provider tests and never contact Neon or need a real key.
Core's restricted PostgreSQL/child tests must separately verify admission,
forced RLS, races, replay/reservation atomicity and native runtime enrollment.

## UI host ABI

The UI deliberately imports the actual Console SDK as a peer. It carries no
copied implementation or ambient mock of authorization hooks. On the EU build
server install the Console's locked dependencies, then run:

```sh
SPITZLI_CONSOLE_ROOT=/path/to/webdock/apps/console node scripts/check-ui.mjs
```

This resolves the host's installed TypeScript, React types and Zod, and checks
this repository's module against its real `src/plugins/sdk.ts`. A host bundles
`ui/index.tsx` under `@spitzli/plugin-ui` and `ui/nav.ts` as the Neon sidebar
metadata; it must grant `databases.discover` before opening the custom Create
form. Run the host's browser fixtures against the real module as well.

## Runtime and deployment

Core mounts a private JSON configuration at the one command-line argument:
`manifest` (absolute finalized manifest path), `issuerPublicKeyHex`, `generation`
and `corePeers` (SDK peer identities). Select exactly one transport pair:

- Private local runtime: `listen` and `core` are absolute Unix socket paths.
- Managed Kubernetes runtime: `listenTls` contains `address`, `certificate`,
  `privateKey`, `authority`; `coreTls` contains `origin`, `domain`, `certificate`,
  `privateKey`, `authority`, `serverCertificateSha256`. Core supplies the exact
  SDK-pinned mTLS configuration and approved runtime peer identity.

Mixed pairs, plaintext TCP and unpinned Core TLS are rejected. Configuration
contains no Neon key, `storageUrl` or `secretKeyHex`. The runtime checks private
file ownership, mode, link count, canonical parents and size. SIGTERM stops the
server and cleans up its owned Unix socket; the process smoke test exercises the
actual binary.

Use Core's existing managed installer with `storage:false`, `sealingKey:false`,
no public route domains and no private configuration overrides. Its existing
agent runtime profile uses UID65532, dropped capabilities, RuntimeDefault
seccomp, read-only root, no service-account token, bounded resources and private
mTLS callbacks. It needs no plugin database or persistent volume. Runtime egress
is only to Core; the Core adapter contacts Neon. The plugin cannot enroll itself,
choose a provider origin, or replace current authority. Installing the source
catalog entry alone does not deploy Core's callback or controller processes.

## Releases and CI

The copied reviewed release scripts pin immutable bases, require the native
validator and exact SDK revision, finalize against the pushed image digest,
reject placeholder images, publish assets without overwriting an existing
release and verify the registry's exact signed receipt against a pinned public
JWK. The finalizer writes `release/tabularium.json`,
`release/neon-universal.json` and `release/release-lock.json`. Once the digest is
known, keep `.tabularium` byte-identical to the final native descriptor as the
source descriptor. Never publish the authoring placeholder.

`.github/workflows/plugin.yml` uses only managed Blacksmith runners and its
managed builder with `nofallback: true`. It stays disabled until the organization
records actual region/tool/builder qualification in the existing `SPITZLI_*`
variables. The owner's own-CI placement exception permits the recorded US West
runner; it does not apply to customer resources or manual builds. Set the normal
scoped registry trust/token and GHCR permissions only in the approved release
environment. Runner labels alone are not evidence of placement or qualification.
No GitHub-hosted or old persistent self-hosted fallback is provided.

Release publication alone does not activate the plugin in Core. Preserve existing
runtime generations and grants; enroll the reviewed digest explicitly, verify
its customer flow and publicly inspect the registry presentation after release.
