const MEGLE_DOCUMENT_TITLE = "<title>Megle</title>";

async function probeMeglePreview(url, fetchImpl) {
  try {
    const response = await fetchImpl(url, { cache: "no-store" });
    if (!response.ok) {
      return { ready: false, detail: `${url} returned ${response.status}` };
    }

    const html = await response.text();
    if (!html.includes(MEGLE_DOCUMENT_TITLE)) {
      return { ready: false, detail: `${url} is not serving the Megle preview` };
    }

    return { ready: true, detail: null };
  } catch (error) {
    return { ready: false, detail: String(error) };
  }
}

export async function isMeglePreviewAvailable(url, fetchImpl = fetch) {
  return (await probeMeglePreview(url, fetchImpl)).ready;
}

export function waitForMeglePreview({
  url,
  child,
  fetchImpl = fetch,
  timeoutMs = 30_000,
  pollIntervalMs = 250
}) {
  return new Promise((resolve, reject) => {
    const deadline = Date.now() + timeoutMs;
    let lastProbeDetail = "preview endpoint has not responded";
    let pollTimer;
    let settled = false;

    const cleanup = () => {
      if (pollTimer) clearTimeout(pollTimer);
      child.off("error", onError);
      child.off("exit", onExit);
    };

    const succeed = () => {
      if (settled) return;
      settled = true;
      cleanup();
      resolve();
    };

    const fail = (error) => {
      if (settled) return;
      settled = true;
      cleanup();
      reject(error);
    };

    const onError = (error) => {
      fail(new Error(
        `Failed to start the Megle preview process for ${url}: ${String(error)}`,
        { cause: error }
      ));
    };

    const onExit = (code, signal) => {
      const exitDetail = signal ? `signal ${signal}` : `code ${code ?? "unknown"}`;
      fail(new Error(
        `Megle preview process exited with ${exitDetail} before ${url} became ready. ` +
        "The fixed preview port may already be in use; stop the conflicting process or reuse an existing Megle preview."
      ));
    };

    const poll = async () => {
      if (settled) return;
      if (child.exitCode !== null) {
        onExit(child.exitCode, child.signalCode);
        return;
      }

      const probe = await probeMeglePreview(url, fetchImpl);
      if (settled) return;

      if (probe.ready) {
        if (child.exitCode !== null) {
          onExit(child.exitCode, child.signalCode);
        } else {
          succeed();
        }
        return;
      }

      lastProbeDetail = probe.detail;
      if (Date.now() >= deadline) {
        fail(new Error(`Timed out waiting for the Megle preview at ${url}: ${lastProbeDetail}`));
        return;
      }

      pollTimer = setTimeout(poll, pollIntervalMs);
    };

    child.once("error", onError);
    child.once("exit", onExit);
    void poll();
  });
}

export async function ensureMeglePreview({
  url,
  spawnPreview,
  fetchImpl = fetch,
  logger = console.log,
  timeoutMs,
  pollIntervalMs
}) {
  if (await isMeglePreviewAvailable(url, fetchImpl)) {
    logger(`[prod] reusing preview server already serving the built UI at ${url}.`);
    return { reused: true, child: null };
  }

  logger(`[prod] serving built UI at ${url} (vite preview)…`);
  const child = spawnPreview();
  await waitForMeglePreview({ url, child, fetchImpl, timeoutMs, pollIntervalMs });
  return { reused: false, child };
}
