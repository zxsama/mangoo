# OneDrive Account Onboarding Verification — 2026-08-26

> Historical evidence note: the publisher-owned application identity described
> below was superseded on 2026-08-29 by the self-managed Client ID workflow.
> Retain these results only as evidence for the OAuth/runtime implementation;
> they do not describe the current release configuration model.

## Scope

This verification covers the OneDrive Client ID configuration, Desktop
loopback authorization, Docker manual response-URI authorization, account
switch/sign-out cleanup, and release-evidence boundaries. The product remains a
public-client Authorization Code + PKCE implementation; no client secret is
accepted or stored.

## Repository evidence

- `npm test`: passed, including Web, Desktop, Core API/client, schema, UI design,
  and Rust suites. The Rust Core run reported 643 passed and 12 intentionally
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
- A fresh immutable-tree release run on implementation snapshot `e618806` passed the clean
  archive, full gate, Desktop loopback emulator, Docker manual emulator,
  Electron authorization URL boundary, sync integration, renderer performance,
  sync benchmark, and Windows OS-backdrop rows. The earlier PDB linker failure
  is superseded by this successful committed-tree run. Evidence is retained at
  `.tmp/sync-release/2026-08-25T18-34-09-752Z-release-662c1329`.

## Required external evidence

The release stays blocked until all of the following are supplied for the exact
release commit:

1. An operator-owned, valid Microsoft Entra public-client registration and
   Client ID configured through the product setup workflow.
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
