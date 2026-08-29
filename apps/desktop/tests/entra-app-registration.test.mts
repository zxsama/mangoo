import assert from "node:assert/strict";
import { describe, it } from "node:test";
import {
  ENTRA_APP_REGISTRATION_URL,
  openEntraAppRegistrationPage
} from "../src/entra-app-registration.ts";

describe("Entra app-registration external link boundary", () => {
  it("opens only the fixed Microsoft Entra application-registration page", async () => {
    const opened: string[] = [];
    assert.equal(
      await openEntraAppRegistrationPage(async (url) => {
        opened.push(url);
      }),
      true
    );
    assert.deepEqual(opened, [
      "https://entra.microsoft.com/#view/Microsoft_AAD_RegisteredApps/ApplicationsListBlade"
    ]);
    assert.equal(opened[0], ENTRA_APP_REGISTRATION_URL);
  });

  it("reports native opener failure without exposing a caller-controlled URL", async () => {
    assert.equal(
      await openEntraAppRegistrationPage(async () => {
        throw new Error("native failure");
      }),
      false
    );
  });
});
