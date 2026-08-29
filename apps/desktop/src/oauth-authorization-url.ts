const MAX_AUTHORIZATION_URL_LENGTH = 8_192;
const MICROSOFT_LOGIN_HOST = "login.microsoftonline.com";
const UUID_PATTERN = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
const REQUIRED_SCOPES = new Set([
  "Files.ReadWrite.AppFolder",
  "offline_access",
  "openid",
  "profile"
]);
const REQUIRED_QUERY_KEYS = new Set([
  "client_id",
  "response_type",
  "redirect_uri",
  "response_mode",
  "scope",
  "state",
  "code_challenge",
  "code_challenge_method"
]);

export type ExternalAuthorizationOpenResult =
  | { ok: true }
  | { ok: false; error: "external_browser_rejected" | "external_browser_open_failed" };

export interface AuthorizationUrlValidationOptions {
  emulatorAuthority?: string;
}

function hasExactlyOneValue(url: URL, key: string): boolean {
  return url.searchParams.getAll(key).length === 1;
}

function isTenantSegment(value: string): boolean {
  return value === "common"
    || value === "organizations"
    || value === "consumers"
    || UUID_PATTERN.test(value)
    || /^(?=.{1,253}$)(?:[A-Za-z0-9](?:[A-Za-z0-9-]{0,61}[A-Za-z0-9])?\.)+[A-Za-z]{2,63}$/.test(value);
}

function isMicrosoftAuthorizationPath(pathname: string): boolean {
  const segments = pathname.split("/").filter(Boolean);
  return segments.length === 4
    && isTenantSegment(segments[0] ?? "")
    && segments[1] === "oauth2"
    && segments[2] === "v2.0"
    && segments[3] === "authorize";
}

function resolveEmulatorAuthorizationEndpoint(authority: string | undefined): URL | null {
  if (!authority) return null;
  try {
    const parsed = new URL(authority);
    if (
      (parsed.protocol !== "http:" && parsed.protocol !== "https:")
      || (parsed.hostname !== "127.0.0.1" && parsed.hostname !== "localhost" && parsed.hostname !== "[::1]")
      || parsed.username
      || parsed.password
      || parsed.search
      || parsed.hash
    ) {
      return null;
    }
    return new URL("oauth2/v2.0/authorize", parsed.href.endsWith("/") ? parsed : `${parsed.href}/`);
  } catch {
    return null;
  }
}

function authorizationEndpointKind(
  url: URL,
  options: AuthorizationUrlValidationOptions
): "microsoft" | "emulator" | null {
  if (
    url.protocol === "https:"
    && url.hostname === MICROSOFT_LOGIN_HOST
    && url.port === ""
    && isMicrosoftAuthorizationPath(url.pathname)
  ) {
    return "microsoft";
  }

  const emulatorEndpoint = resolveEmulatorAuthorizationEndpoint(options.emulatorAuthority);
  return emulatorEndpoint !== null
    && url.origin === emulatorEndpoint.origin
    && url.pathname === emulatorEndpoint.pathname
    ? "emulator"
    : null;
}

function isValidLoopbackRedirect(value: string): boolean {
  try {
    const redirect = new URL(value);
    const port = Number(redirect.port);
    return redirect.protocol === "http:"
      && redirect.hostname === "localhost"
      && Number.isInteger(port)
      && port > 0
      && port <= 65_535
      && redirect.pathname === "/plugin-auth/callback"
      && redirect.username === ""
      && redirect.password === ""
      && redirect.search === ""
      && redirect.hash === "";
  } catch {
    return false;
  }
}

export function isAllowedExternalAuthorizationUrl(
  input: unknown,
  options: AuthorizationUrlValidationOptions = {}
): input is string {
  if (typeof input !== "string" || input.length === 0 || input.length > MAX_AUTHORIZATION_URL_LENGTH) {
    return false;
  }

  let url: URL;
  try {
    url = new URL(input);
  } catch {
    return false;
  }

  const endpointKind = authorizationEndpointKind(url, options);
  if (url.username || url.password || url.hash || endpointKind === null) {
    return false;
  }

  const keys = [...url.searchParams.keys()];
  if (
    keys.length !== REQUIRED_QUERY_KEYS.size
    || keys.some((key) => !REQUIRED_QUERY_KEYS.has(key))
    || [...REQUIRED_QUERY_KEYS].some((key) => !hasExactlyOneValue(url, key))
  ) {
    return false;
  }

  const clientId = url.searchParams.get("client_id") ?? "";
  const state = url.searchParams.get("state") ?? "";
  const challenge = url.searchParams.get("code_challenge") ?? "";
  const scopes = new Set((url.searchParams.get("scope") ?? "").split(/\s+/).filter(Boolean));
  return (endpointKind === "emulator"
      ? /^[A-Za-z0-9._-]{1,128}$/.test(clientId)
      : UUID_PATTERN.test(clientId))
    && url.searchParams.get("response_type") === "code"
    && url.searchParams.get("response_mode") === "query"
    && url.searchParams.get("code_challenge_method") === "S256"
    && /^[A-Za-z0-9_-]{32,256}$/.test(state)
    && /^[A-Za-z0-9_-]{43,128}$/.test(challenge)
    && scopes.size === REQUIRED_SCOPES.size
    && [...REQUIRED_SCOPES].every((scope) => scopes.has(scope))
    && isValidLoopbackRedirect(url.searchParams.get("redirect_uri") ?? "");
}

export async function openExternalAuthorizationUrl(
  input: unknown,
  opener: (url: string) => Promise<void>,
  options: AuthorizationUrlValidationOptions = {},
  onOpened?: () => void
): Promise<ExternalAuthorizationOpenResult> {
  if (!isAllowedExternalAuthorizationUrl(input, options)) {
    return { ok: false, error: "external_browser_rejected" };
  }
  try {
    await opener(input);
    onOpened?.();
    return { ok: true };
  } catch {
    return { ok: false, error: "external_browser_open_failed" };
  }
}
