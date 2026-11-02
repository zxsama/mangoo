# Core Client

Workspace package for the generated TypeScript client based on `contracts/core-api/openapi.yaml`.

Phase 1 keeps `src/generated-contract.ts` as a hand-maintained generated-output placeholder, checked against the OpenAPI document by `npm --workspace @mangoo/core-client run check`.

Do not add app-local Core API DTOs in `apps/web`. Web should import client operations and types from `@mangoo/core-client`, while app-specific runtime config remains in `apps/web/src/core/client.ts`.
