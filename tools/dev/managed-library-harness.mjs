import path from "node:path";

export function managedLibraryHostEnv(dataDir) {
  const hostDir = path.join(dataDir, "host");
  return {
    MEGLE_HOST_DB_PATH: path.join(hostDir, "host.sqlite"),
    MEGLE_LIBRARIES_DIR: path.join(hostDir, "libraries")
  };
}

async function apiRequest(baseUrl, token, apiPath, init = {}) {
  const headers = new Headers(init.headers);
  headers.set("x-megle-session", token);
  if (init.body && !headers.has("content-type")) headers.set("content-type", "application/json");
  const response = await fetch(`${baseUrl}${apiPath}`, { ...init, headers });
  const text = await response.text();
  if (!response.ok) {
    throw new Error(`${apiPath} -> ${response.status} ${response.statusText}: ${text}`);
  }
  return text ? JSON.parse(text) : null;
}

export async function createAndActivateManagedLibrary({
  baseUrl,
  token,
  name,
  managedRoot
}) {
  const preflight = await apiRequest(baseUrl, token, "/libraries/preflight", {
    method: "POST",
    body: JSON.stringify({ name, managedRoot })
  });
  if (!preflight?.ok || !preflight.preflightToken) {
    throw new Error(`managed Library preflight rejected: ${JSON.stringify(preflight)}`);
  }
  const created = await apiRequest(baseUrl, token, "/libraries", {
    method: "POST",
    body: JSON.stringify({
      name,
      managedRoot: preflight.normalizedRoot,
      initialMode: "local-only",
      preflightToken: preflight.preflightToken,
      activate: true
    })
  });
  if (!created?.activated || !created.library?.libraryUuid) {
    throw new Error(`managed Library was not activated: ${JSON.stringify(created)}`);
  }

  // Activation publishes a new atomic context snapshot. Read it back instead
  // of trusting the registry row returned by create, which may precede that
  // final publication by one revision.
  const active = await apiRequest(baseUrl, token, "/libraries/active");
  if (active?.library?.libraryUuid !== created.library.libraryUuid || !Number.isInteger(active.contextRevision)) {
    throw new Error(`active Library did not match creation: ${JSON.stringify(active)}`);
  }
  return { ...created, contextRevision: active.contextRevision };
}

export async function addExistingMediaRoot({
  baseUrl,
  token,
  contextRevision,
  mediaRoot,
  displayName = path.basename(mediaRoot)
}) {
  return apiRequest(baseUrl, token, "/roots", {
    method: "POST",
    headers: { "x-megle-context-revision": String(contextRevision) },
    body: JSON.stringify({ path: mediaRoot, displayName })
  });
}
