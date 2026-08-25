# OneDrive Account Onboarding Verification — 2026-08-26

## Scope

This verification covers the OneDrive Client ID configuration, Desktop
loopback authorization, Docker manual response-URI authorization, account
switch/sign-out cleanup, and release-evidence boundaries. The product remains a
public-client Authorization Code + PKCE implementation; no client secret is
accepted or stored.

## Repository evidence

- `npm test`: passed, including Web, Desktop, Core API/client, schema, UI design,
  and Rust suites. The Rust Core run reported 638 passed and 12 intentionally
  ignored release probes.
- `npm run check:onedrive-bundle`: passed against the generated Windows bundle;
  the bundled manifest hash matched the launched Provider executable.
- `npm run bench:onedrive-provider`: passed with the real framed child and Graph
  emulator, including both loopback polling and manual native-client mode.
- `npm run bench:plugin-runtime`: passed; eight Libraries shared one child and
  browse operations made zero Provider calls.
- `npm run check:sync-integration`: passed after the fake Provider manifest and
  handshake were brought forward to the required `auth.poll`/`auth.cancel`
  method map.
- Quick immutable-tree release run:
  `.tmp/sync-release/onedrive-auth-phase5-final/release-result.json`. The clean
  archive, full gate, Desktop loopback emulator, Docker manual emulator,
  Electron authorization URL boundary, sync integration, renderer performance,
  and sync benchmark rows passed.

The OS-backdrop row could not complete on this workstation because the MSVC
linker reported a PDB filesystem/limit error while rebuilding the Desktop
harness. This is an environment/toolchain failure rather than a failed visual
assertion and must be rerun before release.

## Required external evidence

The release stays blocked until all of the following are supplied for the exact
release commit:

1. A publisher-owned, valid `MEGLE_BUNDLED_ONEDRIVE_CLIENT_ID` and matching
   Microsoft Entra public-client registration.
2. A real Personal account Product OAuth capture plus its Graph transport
   canary.
3. A real Business account Product OAuth capture plus its Graph transport
   canary.
4. A Windows Desktop system-browser/ephemeral-loopback Product OAuth capture.
5. A Docker/Linux manual response-URI Product OAuth capture and container
   SecretStore persistence/restart proof.

Missing paths, credentials, Docker access, or publisher input produce distinct
required `blocked` rows. Emulator results and pre-acquired Graph access tokens
do not substitute for Product OAuth evidence.
