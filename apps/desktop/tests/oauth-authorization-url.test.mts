import assert from "node:assert/strict";
import { describe, it } from "node:test";
import {
  isAllowedExternalAuthorizationUrl,
  openExternalAuthorizationUrl
} from "../src/oauth-authorization-url.ts";

const CLIENT_ID = "11111111-2222-4333-8444-555555555555";
const STATE = "s".repeat(43);
const CHALLENGE = "c".repeat(43);

function authorizationUrl(overrides: Record<string, string> = {}): string {
  const url = new URL("https://login.microsoftonline.com/common/oauth2/v2.0/authorize");
  const values = {
    client_id: CLIENT_ID,
    response_type: "code",
    redirect_uri: "http://127.0.0.1:53682/plugin-auth/callback",
    response_mode: "query",
    scope: "Files.ReadWrite.AppFolder offline_access openid profile",
    state: STATE,
    code_challenge: CHALLENGE,
    code_challenge_method: "S256",
    ...overrides
  };
  for (const [key, value] of Object.entries(values)) url.searchParams.set(key, value);
  return url.href;
}

describe("OneDrive external authorization URL boundary", () => {
  it("accepts the exact Microsoft authorization-code PKCE request", () => {
    assert.equal(isAllowedExternalAuthorizationUrl(authorizationUrl()), true);
    assert.equal(
      isAllowedExternalAuthorizationUrl(
        authorizationUrl().replace("/common/", "/organizations/")
      ),
      true
    );
    assert.equal(
      isAllowedExternalAuthorizationUrl(
        authorizationUrl().replace("/common/", "/example.onmicrosoft.com/")
      ),
      true
    );
  });

  it("rejects protocol, host, path, credentials, fragment, and oversized input", () => {
    const valid = authorizationUrl();
    for (const invalid of [
      valid.replace("https:", "http:"),
      valid.replace("login.microsoftonline.com", "login.microsoftonline.com.evil.example"),
      valid.replace("https://", "https://user@example.com@"),
      valid.replace("/oauth2/v2.0/authorize", "/oauth2/v2.0/token"),
      valid.replace("/common/", "/-/"),
      `${valid}#fragment`,
      `https://login.microsoftonline.com/common/oauth2/v2.0/authorize?${"x".repeat(8_193)}`
    ]) {
      assert.equal(isAllowedExternalAuthorizationUrl(invalid), false, invalid.slice(0, 160));
    }
  });

  it("rejects missing, duplicated, unknown, or weakened OAuth query values", () => {
    const missingState = new URL(authorizationUrl());
    missingState.searchParams.delete("state");
    const duplicateState = new URL(authorizationUrl());
    duplicateState.searchParams.append("state", STATE);
    const unknown = new URL(authorizationUrl());
    unknown.searchParams.set("prompt", "consent");
    const invalidCases = [
      missingState.href,
      duplicateState.href,
      unknown.href,
      authorizationUrl({ response_type: "token" }),
      authorizationUrl({ response_mode: "fragment" }),
      authorizationUrl({ code_challenge_method: "plain" }),
      authorizationUrl({ code_challenge: "short" }),
      authorizationUrl({ scope: "Files.ReadWrite.All offline_access openid profile" }),
      authorizationUrl({ redirect_uri: "http://localhost:53682/plugin-auth/callback" }),
      authorizationUrl({ redirect_uri: "http://127.0.0.1:53682/other" })
    ];
    for (const invalid of invalidCases) {
      assert.equal(isAllowedExternalAuthorizationUrl(invalid), false, invalid);
    }
  });

  it("allows an explicitly configured loopback emulator only when passed by debug code", () => {
    const emulator = authorizationUrl().replace(
      "https://login.microsoftonline.com/common/oauth2/v2.0/authorize",
      "http://127.0.0.1:41001/oauth2/v2.0/authorize"
    ).replace(CLIENT_ID, "emulator-public-client");
    assert.equal(isAllowedExternalAuthorizationUrl(emulator), false);
    assert.equal(
      isAllowedExternalAuthorizationUrl(emulator, {
        emulatorAuthority: "http://127.0.0.1:41001/"
      }),
      true
    );
    assert.equal(
      isAllowedExternalAuthorizationUrl(emulator, {
        emulatorAuthority: "http://emulator.example/"
      }),
      false
    );
  });

  it("reports validation and native opener failures without throwing", async () => {
    let opened: string | null = null;
    assert.deepEqual(
      await openExternalAuthorizationUrl("https://example.com", async (url) => {
        opened = url;
      }),
      { ok: false, error: "external_browser_rejected" }
    );
    assert.equal(opened, null);

    assert.deepEqual(
      await openExternalAuthorizationUrl(authorizationUrl(), async () => {
        throw new Error("native failure");
      }),
      { ok: false, error: "external_browser_open_failed" }
    );

    assert.deepEqual(
      await openExternalAuthorizationUrl(authorizationUrl(), async (url) => {
        opened = url;
      }),
      { ok: true }
    );
    assert.equal(opened, authorizationUrl());
  });
});
