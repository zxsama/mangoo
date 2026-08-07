import { spawn } from "node:child_process";
import { createWriteStream } from "node:fs";
import { mkdir, readdir, readFile, rm, writeFile } from "node:fs/promises";
import http from "node:http";
import path from "node:path";
import process from "node:process";
import WebSocket from "ws";
import { inspectNativeBrowserWindowOptions } from "../../tools/checks/native-browser-window-options.mjs";

const root = path.resolve(process.cwd());
const visualRoot = path.join(root, ".tmp", "visual-check");
const visualRunId = `megle-visual-${Date.now().toString(36)}-${process.pid}`;
const mediaDir = path.join(visualRoot, "media");
const screenshotDir = path.join(visualRoot, "screenshots");
const logDir = path.join(visualRoot, "logs");
const dataDir = path.join(visualRoot, `data-desktop-ui-${visualRunId}`);
const electronUserDataDir = path.join(dataDir, "electron-user-data");
const webUrl = "http://127.0.0.1:5179";
const debugPort = Number(process.env.MEGLE_VISUAL_DEBUG_PORT ?? process.env.MEGLE_REMOTE_DEBUG_PORT ?? 13242);
const runStartedAt = new Date();
const osBackdropEvidenceEnabled = process.env.MEGLE_VISUAL_OS_BACKDROP === "1";
const notificationWindowOnly = process.env.MEGLE_VISUAL_NOTIFICATIONS_ONLY === "1";
const osBackdropEvidenceMode = osBackdropEvidenceEnabled
  ? "required-os-composited"
  : "optional-skipped-static-only";
const osBackdropRequiredCommand =
  '$env:MEGLE_VISUAL_OS_BACKDROP="1"; node .tmp\\visual-check\\desktop-ui-regression.mjs; Remove-Item Env:\\MEGLE_VISUAL_OS_BACKDROP';

const consoleWarnings = [];
const consoleErrors = [];
const networkProblems = [];
const responses = [];
const requestUrls = new Map();
const startupOutput = [];
let child;

function isPreviewApiResponse(item) {
  return item.url.includes("/api/media/") && item.url.includes("/preview");
}

function isThumbnailBlobResponse(item) {
  return item.url.includes("/thumbnail/blob");
}

const desktopMainSource = await readFile(
  path.join(root, "apps", "desktop", "src", "main.ts"),
  "utf8"
);

await mkdir(screenshotDir, { recursive: true });
await mkdir(logDir, { recursive: true });
await mkdir(dataDir, { recursive: true });
await mkdir(electronUserDataDir, { recursive: true });

for (const file of await readdir(screenshotDir).catch(() => [])) {
  await rm(path.join(screenshotDir, file), { force: true });
}

const stdoutPath = path.join(logDir, "desktop-ui-regression.stdout.log");
const stderrPath = path.join(logDir, "desktop-ui-regression.stderr.log");
const summaryPath = path.join(logDir, "desktop-ui-regression-summary.json");
const stdout = createWriteStream(stdoutPath, { flags: "w" });
const stderr = createWriteStream(stderrPath, { flags: "w" });

function appendOutput(source, chunk) {
  const text = chunk.toString();
  startupOutput.push({ source, text });
  if (source === "stderr") stderr.write(text);
  else stdout.write(text);
}

function startDevApp() {
  child = spawn("npm", ["run", "dev"], {
    cwd: root,
    env: {
      ...process.env,
      MEGLE_WEB_URL: webUrl,
      MEGLE_DB_PATH: path.join(dataDir, "megle.sqlite"),
      MEGLE_ELECTRON_USER_DATA_DIR: electronUserDataDir,
      MEGLE_AUTO_ADD_ROOT: mediaDir,
      MEGLE_REMOTE_DEBUG: "1",
      MEGLE_REMOTE_DEBUG_PORT: String(debugPort),
      MEGLE_VISUAL_HARNESS: "1",
      MEGLE_VISUAL_RUN_ID: visualRunId
    },
    shell: process.platform === "win32",
    stdio: ["ignore", "pipe", "pipe"],
    windowsHide: true
  });

  child.stdout.on("data", (chunk) => appendOutput("stdout", chunk));
  child.stderr.on("data", (chunk) => appendOutput("stderr", chunk));
  child.on("exit", (code, signal) => {
    appendOutput(
      "stdout",
      `\n[desktop-ui-regression] npm run dev exited code=${code} signal=${signal ?? "none"}\n`
    );
  });
}

function stopDevApp() {
  if (!child || child.killed || child.pid === undefined) return Promise.resolve();
  return new Promise((resolve) => {
    if (process.platform === "win32") {
      const killer = spawn("taskkill", ["/PID", String(child.pid), "/T", "/F"], {
        stdio: "ignore",
        windowsHide: true
      });
      killer.on("error", () => {
        child.kill();
        resolve();
      });
      killer.on("exit", () => resolve());
      return;
    }
    child.kill("SIGTERM");
    setTimeout(() => {
      if (!child.killed) child.kill("SIGKILL");
      resolve();
    }, 2000).unref();
  });
}

async function httpJson(url) {
  return new Promise((resolve, reject) => {
    const request = http.get(url, (response) => {
      let body = "";
      response.setEncoding("utf8");
      response.on("data", (chunk) => {
        body += chunk;
      });
      response.on("end", () => {
        try {
          resolve(JSON.parse(body));
        } catch (error) {
          reject(error);
        }
      });
    });
    request.on("error", reject);
    request.setTimeout(1000, () => request.destroy(new Error(`Timed out fetching ${url}`)));
  });
}

async function waitForTarget(
  predicate = (target) => target.url.startsWith(webUrl) && !target.url.includes("#/"),
  timeoutMs = 90000
) {
  const deadline = Date.now() + timeoutMs;
  let lastError;
  while (Date.now() < deadline) {
    try {
      const targets = await httpJson(`http://127.0.0.1:${debugPort}/json/list`);
      const page = targets.find((target) => target.type === "page" && predicate(target));
      if (page?.webSocketDebuggerUrl) return page;
    } catch (error) {
      lastError = error;
    }
    await delay(250);
  }
  throw new Error(`Timed out waiting for Electron CDP target: ${String(lastError)}`);
}

async function matchingTargets(predicate) {
  const targets = await httpJson(`http://127.0.0.1:${debugPort}/json/list`);
  return targets.filter((target) => target.type === "page" && predicate(target));
}

async function waitForTargetCount(predicate, expectedCount, timeoutMs = 10000) {
  const deadline = Date.now() + timeoutMs;
  let lastCount = -1;
  while (Date.now() < deadline) {
    const targets = await matchingTargets(predicate);
    lastCount = targets.length;
    if (lastCount === expectedCount) return targets;
    await delay(200);
  }
  throw new Error(`Timed out waiting for target count ${expectedCount}; last=${lastCount}`);
}

class CdpClient {
  constructor(wsUrl) {
    this.ws = new WebSocket(wsUrl);
    this.nextId = 1;
    this.pending = new Map();
    this.events = new Map();
    this.ready = new Promise((resolve, reject) => {
      this.ws.once("open", resolve);
      this.ws.once("error", reject);
    });
    this.ws.on("message", (raw) => this.#handleMessage(raw));
    this.ws.on("error", (error) => {
      for (const { reject } of this.pending.values()) reject(error);
      this.pending.clear();
    });
  }

  #handleMessage(raw) {
    const message = JSON.parse(raw.toString());
    if (message.id) {
      const pending = this.pending.get(message.id);
      if (!pending) return;
      this.pending.delete(message.id);
      if (message.error) pending.reject(new Error(`${pending.method}: ${JSON.stringify(message.error)}`));
      else pending.resolve(message.result ?? {});
      return;
    }
    if (message.method) {
      for (const handler of this.events.get(message.method) ?? []) handler(message.params ?? {});
    }
  }

  on(method, handler) {
    this.events.set(method, [...(this.events.get(method) ?? []), handler]);
  }

  send(method, params = {}, timeoutMs = 30000) {
    const id = this.nextId++;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        if (!this.pending.has(id)) return;
        this.pending.delete(id);
        reject(new Error(`CDP timeout: ${method}`));
      }, timeoutMs);
      timer.unref();
      this.pending.set(id, {
        method,
        resolve: (value) => {
          clearTimeout(timer);
          resolve(value);
        },
        reject: (error) => {
          clearTimeout(timer);
          reject(error);
        }
      });
      try {
        this.ws.send(JSON.stringify({ id, method, params }));
      } catch (error) {
        this.pending.delete(id);
        clearTimeout(timer);
        reject(error);
      }
    });
  }

  close() {
    this.ws.close();
  }
}

function delay(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

async function evaluate(client, expression) {
  const result = await client.send("Runtime.evaluate", {
    expression,
    awaitPromise: true,
    returnByValue: true
  });
  if (result.exceptionDetails) throw new Error(JSON.stringify(result.exceptionDetails));
  return result.result?.value;
}

function fireRendererCommand(client, expression) {
  void client.send("Runtime.evaluate", {
    expression,
    awaitPromise: false,
    returnByValue: false
  }).catch(() => undefined);
}

async function waitFor(client, expression, timeoutMs = 30000, label = expression) {
  const deadline = Date.now() + timeoutMs;
  let lastValue;
  while (Date.now() < deadline) {
    lastValue = await evaluate(client, expression);
    if (lastValue) return lastValue;
    await delay(200);
  }
  throw new Error(`Timed out waiting for ${label}; last=${JSON.stringify(lastValue)}`);
}

async function screenshot(client, name) {
  await delay(220);
  await client.send("Page.bringToFront").catch(() => undefined);
  const bridgeReady = await evaluate(
    client,
    `typeof window.megleDesktop?.visual?.capturePage === "function"`
  );
  if (!bridgeReady) {
    throw new Error(
      "Desktop visual page capture bridge is unavailable; ensure MEGLE_VISUAL_HARNESS=1 and the preload exposed window.megleDesktop.visual.capturePage"
    );
  }
  const data = await evaluate(
    client,
    `window.megleDesktop.visual.capturePage()`
  );
  if (typeof data !== "string" || data.length === 0) {
    throw new Error("Desktop visual page capture did not return screenshot data");
  }
  const filePath = path.join(screenshotDir, name);
  await writeFile(filePath, Buffer.from(data, "base64"));
  return filePath;
}

async function rendererScreenshot(client, name) {
  await delay(220);
  await client.send("Page.bringToFront").catch(() => undefined);
  const capture = await client.send("Page.captureScreenshot", {
    captureBeyondViewport: false,
    format: "png"
  });
  if (typeof capture.data !== "string" || capture.data.length === 0) {
    throw new Error("Renderer screenshot did not return PNG data");
  }
  const filePath = path.join(screenshotDir, name);
  await writeFile(filePath, Buffer.from(capture.data, "base64"));
  return filePath;
}

async function resizeScriptOpenedWindow(client, width, height) {
  await evaluate(
    client,
    `(() => { window.resizeTo(${Math.round(width)}, ${Math.round(height)}); return true; })()`
  );
  await delay(320);
  return evaluate(
    client,
    `({ innerWidth: window.innerWidth, innerHeight: window.innerHeight, outerWidth: window.outerWidth, outerHeight: window.outerHeight })`
  );
}

async function captureWindowBackdropEvidence(client, name, layout) {
  if (!osBackdropEvidenceEnabled) {
    return {
      enabled: false,
      skipped: true,
      mode: osBackdropEvidenceMode,
      requiredCommand: osBackdropRequiredCommand,
      reason:
        "Skipped optional OS-composited acrylic proof; this default run uses static source/options plus renderer screenshot evidence only. Set MEGLE_VISUAL_OS_BACKDROP=1 for required OS backdrop capture."
    };
  }
  const evidence = await captureWindowBackdropEvidenceViaWindows(name, layout);
  return {
    enabled: true,
    mode: osBackdropEvidenceMode,
    requiredCommand: osBackdropRequiredCommand,
    ...evidence
  };
}

function osBackdropVerificationSummary(windowBackdrop) {
  if (!osBackdropEvidenceEnabled) {
    return {
      mode: osBackdropEvidenceMode,
      required: false,
      status: "skipped",
      command: osBackdropRequiredCommand,
      note: "Default visual harness did not capture the OS-composited acrylic backdrop; release verification must run the required mode for native pixel evidence."
    };
  }

  if (
    !windowBackdrop ||
    windowBackdrop.skipped ||
    !windowBackdrop.path ||
    typeof windowBackdrop.maxDelta !== "number" ||
    typeof windowBackdrop.bottomMaxDelta !== "number" ||
    typeof windowBackdrop.uiSurfaceDelta !== "number"
  ) {
    return {
      mode: osBackdropEvidenceMode,
      required: true,
      status: "failed",
      command: osBackdropRequiredCommand,
      note: "Required OS-composited acrylic backdrop evidence was unavailable or incomplete."
    };
  }

  const pixelEvidencePassed = backdropPixelEvidencePassed(windowBackdrop);
  return {
    mode: osBackdropEvidenceMode,
    required: true,
    status: pixelEvidencePassed ? "passed" : "failed",
    command: osBackdropRequiredCommand,
    note: pixelEvidencePassed
      ? "OS screenshot pixel evidence shows transparent passthrough or native acrylic-tinted desktop variation while Megle UI surfaces remain visible over the backdrop."
      : "OS screenshot pixel evidence did not satisfy transparent/acrylic backdrop and visible-surface thresholds."
  };
}

function backdropPixelEvidencePassed(windowBackdrop) {
  const uiSurfaceVisible = windowBackdrop.uiSurfaceDelta >= 18;
  return (
    uiSurfaceVisible &&
    (rawTransparentEdgeEvidencePassed(windowBackdrop) || acrylicBackdropEvidencePassed(windowBackdrop))
  );
}

function rawTransparentEdgeEvidencePassed(windowBackdrop) {
  return transparentCornerSampleCount(windowBackdrop) >= 3 && windowBackdrop.bottomMaxDelta <= 55;
}

function acrylicBackdropEvidencePassed(windowBackdrop) {
  const insideSamples = [...(windowBackdrop.samples ?? []), ...(windowBackdrop.bottomSamples ?? [])]
    .map((sample) => sample.inside)
    .filter(Boolean);
  if (insideSamples.length < 4) return false;

  const luminanceValues = insideSamples.map((color) => colorLuminance(color));
  const min = Math.min(...luminanceValues);
  const max = Math.max(...luminanceValues);
  const average = luminanceValues.reduce((sum, value) => sum + value, 0) / luminanceValues.length;
  return max - min >= 10 && average > 30 && average < 235;
}

function colorLuminance(color) {
  return 0.2126 * color.r + 0.7152 * color.g + 0.0722 * color.b;
}

function transparentCornerSampleCount(windowBackdrop) {
  return (windowBackdrop.samples ?? []).filter((sample) => sample.delta <= 55).length;
}

async function analyzeWindowBackdropScreenshot(filePath, margin, viewport) {
  const escapedPath = filePath.replace(/'/g, "''");
  const script = `
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing
$path = '${escapedPath}'
$marginDip = ${margin}
$windowWidthDip = ${Math.round(viewport.width)}
$windowHeightDip = ${Math.round(viewport.height)}
$bitmap = [System.Drawing.Bitmap]::FromFile($path)
try {
  $scaleX = $bitmap.Width / ($windowWidthDip + (2 * $marginDip))
  $scaleY = $bitmap.Height / ($windowHeightDip + (2 * $marginDip))
  $marginX = [Math]::Round($marginDip * $scaleX)
  $marginY = [Math]::Round($marginDip * $scaleY)
  $windowWidth = [Math]::Round($windowWidthDip * $scaleX)
  $windowHeight = [Math]::Round($windowHeightDip * $scaleY)
  $insideInsetX = [Math]::Max(3, [Math]::Round(8 * $scaleX))
  $insideInsetY = [Math]::Max(3, [Math]::Round(8 * $scaleY))
  $outsideInsetX = [Math]::Max(4, [Math]::Round(10 * $scaleX))
  $outsideInsetY = [Math]::Max(4, [Math]::Round(10 * $scaleY))
  $patchRadius = [Math]::Max(2, [Math]::Round(3 * [Math]::Max($scaleX, $scaleY)))

  function AveragePatch($image, [int]$centerX, [int]$centerY, [int]$radius) {
    if ($centerX -lt 0 -or $centerX -ge $image.Width -or $centerY -lt 0 -or $centerY -ge $image.Height) {
      throw "sample point outside screenshot: $centerX,$centerY"
    }
    $sumR = 0
    $sumG = 0
    $sumB = 0
    $count = 0
    for ($y = [Math]::Max(0, $centerY - $radius); $y -le [Math]::Min($image.Height - 1, $centerY + $radius); $y++) {
      for ($x = [Math]::Max(0, $centerX - $radius); $x -le [Math]::Min($image.Width - 1, $centerX + $radius); $x++) {
        $pixel = $image.GetPixel($x, $y)
        $sumR += $pixel.R
        $sumG += $pixel.G
        $sumB += $pixel.B
        $count += 1
      }
    }
    return [pscustomobject]@{
      r = [Math]::Round($sumR / $count, 2)
      g = [Math]::Round($sumG / $count, 2)
      b = [Math]::Round($sumB / $count, 2)
    }
  }

  function ColorDistance($a, $b) {
    return [Math]::Sqrt(
      [Math]::Pow($a.r - $b.r, 2) +
      [Math]::Pow($a.g - $b.g, 2) +
      [Math]::Pow($a.b - $b.b, 2)
    )
  }

  function CompareSample($name, [int]$insideX, [int]$insideY, [int]$outsideX, [int]$outsideY) {
    $inside = AveragePatch $bitmap $insideX $insideY $patchRadius
    $outside = AveragePatch $bitmap $outsideX $outsideY $patchRadius
    return [pscustomobject]@{
      name = $name
      inside = $inside
      outside = $outside
      delta = [Math]::Round((ColorDistance $inside $outside), 2)
    }
  }

  $cornerSamples = @(
    CompareSample 'topLeft' ($marginX + $insideInsetX) ($marginY + $insideInsetY) ($marginX - $outsideInsetX) ($marginY + $insideInsetY)
    CompareSample 'topRight' ($marginX + $windowWidth - $insideInsetX - 1) ($marginY + $insideInsetY) ($marginX + $windowWidth + $outsideInsetX) ($marginY + $insideInsetY)
    CompareSample 'bottomLeft' ($marginX + $insideInsetX) ($marginY + $windowHeight - $insideInsetY - 1) ($marginX - $outsideInsetX) ($marginY + $windowHeight - $insideInsetY - 1)
    CompareSample 'bottomRight' ($marginX + $windowWidth - $insideInsetX - 1) ($marginY + $windowHeight - $insideInsetY - 1) ($marginX + $windowWidth + $outsideInsetX) ($marginY + $windowHeight - $insideInsetY - 1)
  )
  $uiSamples = @(
    CompareSample 'titlebarCenterSurface' ($marginX + [Math]::Round($windowWidth * 0.5)) ($marginY + [Math]::Round(28 * $scaleY)) ($marginX + [Math]::Round($windowWidth * 0.5)) ($marginY - $outsideInsetY)
    CompareSample 'sidebarSurface' ($marginX + [Math]::Round(42 * $scaleX)) ($marginY + [Math]::Round(140 * $scaleY)) ($marginX - $outsideInsetX) ($marginY + [Math]::Round(140 * $scaleY))
    CompareSample 'inspectorSurface' ($marginX + $windowWidth - [Math]::Round(42 * $scaleX)) ($marginY + [Math]::Round(140 * $scaleY)) ($marginX + $windowWidth + $outsideInsetX) ($marginY + [Math]::Round(140 * $scaleY))
  )

  $cornerMaxDelta = 0
  foreach ($sample in $cornerSamples) {
    if ($sample.delta -gt $cornerMaxDelta) { $cornerMaxDelta = $sample.delta }
  }
  $uiMaxDelta = 0
  foreach ($sample in $uiSamples) {
    if ($sample.delta -gt $uiMaxDelta) { $uiMaxDelta = $sample.delta }
  }

  [pscustomobject]@{
    path = $path
    margin = $marginDip
    image = @{ width = $bitmap.Width; height = $bitmap.Height }
    inferredScale = @{ x = [Math]::Round($scaleX, 3); y = [Math]::Round($scaleY, 3) }
    samples = $cornerSamples
    uiSamples = $uiSamples
    maxDelta = [Math]::Round($cornerMaxDelta, 2)
    uiSurfaceDelta = [Math]::Round($uiMaxDelta, 2)
  } | ConvertTo-Json -Depth 8 -Compress
} finally {
  $bitmap.Dispose()
}
`;

  return new Promise((resolve, reject) => {
    const ps = spawn(
      "powershell.exe",
      ["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", script],
      { stdio: ["ignore", "pipe", "pipe"], windowsHide: true }
    );
    let output = "";
    let errorText = "";
    ps.stdout.on("data", (chunk) => {
      output += chunk.toString();
    });
    ps.stderr.on("data", (chunk) => {
      errorText += chunk.toString();
    });
    ps.on("error", reject);
    ps.on("exit", (code) => {
      if (code !== 0) {
        reject(new Error(errorText.trim() || `Backdrop screenshot analysis failed with exit code ${code}`));
        return;
      }
      try {
        resolve(JSON.parse(output));
      } catch (error) {
        reject(new Error(`Backdrop screenshot analysis did not return JSON: ${String(error)}; output=${output}`));
      }
    });
  });
}

async function captureWindowBackdropEvidenceViaWindows(name) {
  if (process.platform !== "win32") {
    return {
      skipped: true,
      reason: "window backdrop pixel evidence requires Windows desktop capture"
    };
  }

  const filePath = path.join(screenshotDir, name);
  const escapedPath = filePath.replace(/'/g, "''");
  const escapedRunId = visualRunId.replace(/'/g, "''");
  const margin = 28;
  const script = `
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing
Add-Type @"
using System;
using System.Runtime.InteropServices;
public class MegleBackdropCapture {
  [StructLayout(LayoutKind.Sequential)]
  public struct RECT {
    public int Left;
    public int Top;
    public int Right;
    public int Bottom;
  }
  [DllImport("user32.dll")]
  public static extern bool GetWindowRect(IntPtr hWnd, out RECT rect);
  [DllImport("user32.dll")]
  public static extern bool IsWindow(IntPtr hWnd);
  [DllImport("user32.dll")]
  public static extern bool SetForegroundWindow(IntPtr hWnd);
  [DllImport("user32.dll")]
  public static extern bool ShowWindow(IntPtr hWnd, int nCmdShow);
  [DllImport("user32.dll")]
  public static extern bool SetWindowPos(IntPtr hWnd, IntPtr hWndInsertAfter, int X, int Y, int cx, int cy, uint uFlags);
  [DllImport("user32.dll")]
  public static extern bool MoveWindow(IntPtr hWnd, int X, int Y, int nWidth, int nHeight, bool bRepaint);
}
"@
$path = '${escapedPath}'
$runId = '${escapedRunId}'
$margin = ${margin}
$processInfos = Get-CimInstance Win32_Process | Where-Object {
  $_.CommandLine -and (
    $_.CommandLine -like '*apps/desktop/dist/main.js*' -or
    $_.CommandLine -like '*apps\\desktop\\dist\\main.js*'
  ) -and
  $_.CommandLine -like '*--megle-visual-harness=1*' -and
  $_.CommandLine -like "*--megle-visual-run-id=$runId*" -and
  $_.CommandLine -notlike '* --type=*' -and
  $_.Name -notin @('powershell.exe', 'pwsh.exe', 'node.exe')
} | Sort-Object ProcessId
$window = $null
foreach ($processInfo in $processInfos) {
  $window = Get-Process -Id $processInfo.ProcessId -ErrorAction SilentlyContinue
  if ($window -and $window.MainWindowHandle -ne 0) {
    break
  }
  $window = $null
}
if (-not $window) { throw 'Megle Electron window not found for backdrop capture' }
$topMost = [IntPtr]::new(-1)
$notTopMost = [IntPtr]::new(-2)
$swpNoMove = 0x0002
$swpNoSize = 0x0001
$swpShowWindow = 0x0040
$rect = New-Object MegleBackdropCapture+RECT
$handle = [IntPtr]::Zero
$rectReady = $false
for ($attempt = 0; $attempt -lt 20; $attempt++) {
  $window.Refresh()
  $candidate = $window.MainWindowHandle
  if ($candidate -ne 0 -and [MegleBackdropCapture]::IsWindow($candidate)) {
    $handle = $candidate
    [MegleBackdropCapture]::ShowWindow($handle, 9) | Out-Null
    [MegleBackdropCapture]::SetForegroundWindow($handle) | Out-Null
    [MegleBackdropCapture]::SetWindowPos($handle, $topMost, 0, 0, 0, 0, ($swpNoMove -bor $swpNoSize -bor $swpShowWindow)) | Out-Null
    Start-Sleep -Milliseconds 120
    if ([MegleBackdropCapture]::GetWindowRect($handle, [ref] $rect)) {
      $rectReady = $true
      break
    }
  }
  Start-Sleep -Milliseconds 100
}
if (-not $rectReady) {
  throw "GetWindowRect failed before backdrop capture: handle=$($handle.ToInt64()) isWindow=$([MegleBackdropCapture]::IsWindow($handle))"
}
$windowWidth = [Math]::Max(1, $rect.Right - $rect.Left)
$windowHeight = [Math]::Max(1, $rect.Bottom - $rect.Top)
if ($rect.Left -lt ($margin + 12) -or $rect.Top -lt ($margin + 12)) {
  [MegleBackdropCapture]::MoveWindow($handle, 72, 72, $windowWidth, $windowHeight, $true) | Out-Null
  Start-Sleep -Milliseconds 260
  if (-not [MegleBackdropCapture]::GetWindowRect($handle, [ref] $rect)) {
    throw 'GetWindowRect failed after moving window for backdrop capture'
  }
  $windowWidth = [Math]::Max(1, $rect.Right - $rect.Left)
  $windowHeight = [Math]::Max(1, $rect.Bottom - $rect.Top)
}
$captureX = [Math]::Max(0, $rect.Left - $margin)
$captureY = [Math]::Max(0, $rect.Top - $margin)
$leftMargin = $rect.Left - $captureX
$topMargin = $rect.Top - $captureY
$captureWidth = $windowWidth + $leftMargin + $margin
$captureHeight = $windowHeight + $topMargin + $margin
[MegleBackdropCapture]::SetWindowPos($handle, $topMost, 0, 0, 0, 0, ($swpNoMove -bor $swpNoSize -bor $swpShowWindow)) | Out-Null
Start-Sleep -Milliseconds 260
$bitmap = New-Object System.Drawing.Bitmap $captureWidth, $captureHeight
$graphics = [System.Drawing.Graphics]::FromImage($bitmap)
try {
  $graphics.CopyFromScreen($captureX, $captureY, 0, 0, $bitmap.Size)
  $bitmap.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)

  function AveragePatch($image, [int]$centerX, [int]$centerY, [int]$radius) {
    $sumR = 0
    $sumG = 0
    $sumB = 0
    $count = 0
    for ($y = [Math]::Max(0, $centerY - $radius); $y -le [Math]::Min($image.Height - 1, $centerY + $radius); $y++) {
      for ($x = [Math]::Max(0, $centerX - $radius); $x -le [Math]::Min($image.Width - 1, $centerX + $radius); $x++) {
        $pixel = $image.GetPixel($x, $y)
        $sumR += $pixel.R
        $sumG += $pixel.G
        $sumB += $pixel.B
        $count += 1
      }
    }
    return [pscustomobject]@{
      r = [Math]::Round($sumR / $count, 2)
      g = [Math]::Round($sumG / $count, 2)
      b = [Math]::Round($sumB / $count, 2)
    }
  }

  function ColorDistance($a, $b) {
    return [Math]::Sqrt(
      [Math]::Pow($a.r - $b.r, 2) +
      [Math]::Pow($a.g - $b.g, 2) +
      [Math]::Pow($a.b - $b.b, 2)
    )
  }

  function CompareSample($name, [int]$insideX, [int]$insideY, [int]$outsideX, [int]$outsideY) {
    $inside = AveragePatch $bitmap $insideX $insideY $patchRadius
    $outside = AveragePatch $bitmap $outsideX $outsideY $patchRadius
    return [pscustomobject]@{
      name = $name
      inside = $inside
      outside = $outside
      delta = [Math]::Round((ColorDistance $inside $outside), 2)
    }
  }

  function MaxDelta($samples) {
    $maxDelta = 0
    foreach ($sample in $samples) {
      if ($sample.delta -gt $maxDelta) { $maxDelta = $sample.delta }
    }
    return [Math]::Round($maxDelta, 2)
  }

  $insideInset = 8
  $outsideInset = 10
  $patchRadius = 3
  $cornerSamples = @(
    CompareSample 'topLeft' ($leftMargin + $insideInset) ($topMargin + $insideInset) ($leftMargin - $outsideInset) ($topMargin + $insideInset)
    CompareSample 'topRight' ($leftMargin + $windowWidth - $insideInset - 1) ($topMargin + $insideInset) ($leftMargin + $windowWidth + $outsideInset) ($topMargin + $insideInset)
    CompareSample 'bottomLeft' ($leftMargin + $insideInset) ($topMargin + $windowHeight - $insideInset - 1) ($leftMargin - $outsideInset) ($topMargin + $windowHeight - $insideInset - 1)
    CompareSample 'bottomRight' ($leftMargin + $windowWidth - $insideInset - 1) ($topMargin + $windowHeight - $insideInset - 1) ($leftMargin + $windowWidth + $outsideInset) ($topMargin + $windowHeight - $insideInset - 1)
  )
  $bottomSamples = @(
    CompareSample 'bottomCenter' ($leftMargin + [Math]::Round($windowWidth * 0.5)) ($topMargin + $windowHeight - $insideInset - 1) ($leftMargin + [Math]::Round($windowWidth * 0.5)) ($topMargin + $windowHeight + $outsideInset)
  )
  $uiSamples = @(
    CompareSample 'titlebarCenterSurface' ($leftMargin + [Math]::Round($windowWidth * 0.5)) ($topMargin + 28) ($leftMargin + [Math]::Round($windowWidth * 0.5)) ($topMargin - $outsideInset)
    CompareSample 'sidebarSurface' ($leftMargin + 42) ($topMargin + 140) ($leftMargin - $outsideInset) ($topMargin + 140)
    CompareSample 'inspectorSurface' ($leftMargin + $windowWidth - 42) ($topMargin + 140) ($leftMargin + $windowWidth + $outsideInset) ($topMargin + 140)
  )

  [pscustomobject]@{
    path = $path
    runId = $runId
    capturedAtUtc = [DateTime]::UtcNow.ToString("o")
    process = @{
      id = $window.Id
      name = $window.ProcessName
      title = $window.MainWindowTitle
      handle = $handle.ToInt64()
      commandLine = $processInfo.CommandLine
    }
    margin = $margin
    image = @{
      width = $bitmap.Width
      height = $bitmap.Height
    }
    window = @{
      x = $rect.Left
      y = $rect.Top
      width = $windowWidth
      height = $windowHeight
    }
    samples = $cornerSamples
    bottomSamples = $bottomSamples
    uiSamples = $uiSamples
    maxDelta = MaxDelta $cornerSamples
    bottomMaxDelta = MaxDelta $bottomSamples
    uiSurfaceDelta = MaxDelta $uiSamples
  } | ConvertTo-Json -Depth 8 -Compress
} finally {
  if ($handle -ne [IntPtr]::Zero -and [MegleBackdropCapture]::IsWindow($handle)) {
    [MegleBackdropCapture]::SetWindowPos($handle, $notTopMost, 0, 0, 0, 0, ($swpNoMove -bor $swpNoSize -bor $swpShowWindow)) | Out-Null
  }
  if ($graphics) { $graphics.Dispose() }
  if ($bitmap) { $bitmap.Dispose() }
}
`;

  return new Promise((resolve, reject) => {
    const ps = spawn(
      "powershell.exe",
      ["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", script],
      { stdio: ["ignore", "pipe", "pipe"], windowsHide: true }
    );
    let output = "";
    let errorText = "";
    ps.stdout.on("data", (chunk) => {
      output += chunk.toString();
    });
    ps.stderr.on("data", (chunk) => {
      errorText += chunk.toString();
    });
    ps.on("error", reject);
    ps.on("exit", (code) => {
      if (code !== 0) {
        reject(new Error(errorText.trim() || `Backdrop capture failed with exit code ${code}`));
        return;
      }
      try {
        resolve(JSON.parse(output));
      } catch (error) {
        reject(new Error(`Backdrop capture did not return JSON: ${String(error)}; output=${output}`));
      }
    });
  });
}

async function mouse(client, type, params) {
  await client.send("Input.dispatchMouseEvent", {
    type,
    x: Math.round(params.x),
    y: Math.round(params.y),
    button: params.button ?? "none",
    clickCount: params.clickCount ?? 0,
    pointerType: "mouse"
  });
}

async function mediaTilePoint(client, name) {
  const point = await evaluate(
    client,
    `(() => {
      const labels = [...document.querySelectorAll(".media-tile .tile-label")];
      const label = labels.find((item) => item.textContent?.trim()?.includes(${JSON.stringify(name)}));
      const button = label?.closest(".media-tile");
      if (!button) return null;
      const rect = button.getBoundingClientRect();
      return { x: rect.left + rect.width / 2, y: rect.top + rect.height * 0.42 };
    })()`
  );
  if (!point) throw new Error(`Media tile not found: ${name}`);
  return point;
}

async function selectMediaByName(client, name) {
  const selected = await evaluate(
    client,
    `(() => {
      const labels = [...document.querySelectorAll(".media-tile .tile-label")];
      const label = labels.find((item) => item.textContent?.trim()?.includes(${JSON.stringify(name)}));
      const button = label?.closest(".media-tile");
      button?.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true, view: window, detail: 1 }));
      return Boolean(button);
    })()`
  );
  if (!selected) throw new Error(`Media tile not found: ${name}`);
  await delay(350);
}

async function doubleClickMediaByName(client, name) {
  const opened = await evaluate(
    client,
    `(() => {
      const labels = [...document.querySelectorAll(".media-tile .tile-label")];
      const label = labels.find((item) => item.textContent?.trim()?.includes(${JSON.stringify(name)}));
      const button = label?.closest(".media-tile");
      if (!button) return false;
      button.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true, view: window, detail: 1 }));
      button.dispatchEvent(new MouseEvent("dblclick", { bubbles: true, cancelable: true, view: window, detail: 2 }));
      return new Promise((resolve) => {
        window.requestAnimationFrame(() => {
          window.requestAnimationFrame(() => resolve(true));
        });
      });
    })()`
  );
  if (!opened) throw new Error(`Media tile not found: ${name}`);
  await delay(400);
}

async function doubleClickStage(client, xRatio = 0.56, yRatio = 0.47) {
  const point = await pointFor(client, ".central-preview-stage", xRatio, yRatio);
  const anchorBefore = await imagePointRatioAt(client, point);
  await doubleClickPoint(client, point);
  const anchorAfter = await imagePointRatioAt(client, point);
  return { point, anchorBefore, anchorAfter };
}

async function doubleClickPoint(client, point) {
  await mouse(client, "mouseMoved", point);
  await mouse(client, "mousePressed", { ...point, button: "left", clickCount: 1 });
  await mouse(client, "mouseReleased", { ...point, button: "left", clickCount: 1 });
  await delay(80);
  await mouse(client, "mousePressed", { ...point, button: "left", clickCount: 2 });
  await mouse(client, "mouseReleased", { ...point, button: "left", clickCount: 2 });
  await delay(400);
}

async function pointFor(client, selector, xRatio = 0.5, yRatio = 0.5) {
  const point = await evaluate(
    client,
    `(() => {
      const element = document.querySelector(${JSON.stringify(selector)});
      if (!element) return null;
      const rect = element.getBoundingClientRect();
      return { x: rect.left + rect.width * ${xRatio}, y: rect.top + rect.height * ${yRatio} };
    })()`
  );
  if (!point) throw new Error(`Selector not found: ${selector}`);
  return point;
}

async function forcePseudoState(client, selector, forcedPseudoClasses) {
  await Promise.all([
    client.send("DOM.enable"),
    client.send("CSS.enable")
  ]);
  const { root } = await client.send("DOM.getDocument", { depth: -1, pierce: true });
  const { nodeId } = await client.send("DOM.querySelector", {
    nodeId: root.nodeId,
    selector
  });
  if (!nodeId) throw new Error(`Selector not found for pseudo state: ${selector}`);
  await client.send("CSS.forcePseudoState", { nodeId, forcedPseudoClasses });
}

async function moveToSelector(client, selector, xRatio, yRatio) {
  const point = await pointFor(client, selector, xRatio, yRatio);
  await mouse(client, "mouseMoved", point);
  await delay(40);
  await evaluate(
    client,
    `(() => new Promise((resolve) => {
      window.focus();
      window.dispatchEvent(new PointerEvent("pointermove", {
        bubbles: true,
        clientX: ${point.x},
        clientY: ${point.y},
        pointerType: "mouse"
      }));
      window.dispatchEvent(new MouseEvent("mousemove", {
        bubbles: true,
        clientX: ${point.x},
        clientY: ${point.y}
      }));
      window.requestAnimationFrame(() => {
        window.requestAnimationFrame(() => resolve(true));
      });
    }))()`
  );
  return point;
}

async function clickSelector(client, selector, xRatio = 0.5, yRatio = 0.5) {
  await client.send("Page.bringToFront").catch(() => undefined);
  await evaluate(client, "window.focus(); true").catch(() => undefined);
  const point = await pointFor(client, selector, xRatio, yRatio);
  await mouse(client, "mouseMoved", point);
  await mouse(client, "mousePressed", { ...point, button: "left", clickCount: 1 });
  await mouse(client, "mouseReleased", { ...point, button: "left", clickCount: 1 });
  await delay(250);
  return point;
}

async function titlebarCenterBlankPoint(client) {
  const point = await evaluate(
    client,
    `(() => {
      const titlebar = document.querySelector(".shell-titlebar-center");
      if (!titlebar) return null;
      const rect = titlebar.getBoundingClientRect();
      const candidates = [
        { x: rect.left + rect.width * 0.5, y: rect.top + rect.height * 0.08 },
        { x: rect.left + rect.width * 0.75, y: rect.top + rect.height * 0.08 },
        { x: rect.left + rect.width * 0.25, y: rect.top + rect.height * 0.08 },
        { x: rect.left + rect.width * 0.94, y: rect.top + rect.height * 0.5 },
        { x: rect.left + rect.width * 0.82, y: rect.top + rect.height * 0.5 },
        { x: rect.left + rect.width * 0.06, y: rect.top + rect.height * 0.5 }
      ];
      const interactiveSelector = "button,input,select,textarea,a,[role='button'],[role='tab'],[role='tablist'],[role='group']";
      const usable = candidates.find((candidate) => {
        const element = document.elementFromPoint(candidate.x, candidate.y);
        return element && titlebar.contains(element) && !element.closest(interactiveSelector);
      }) ?? candidates[0];
      const element = document.elementFromPoint(usable.x, usable.y);
      const elementStyle = element ? getComputedStyle(element) : null;
      const titlebarStyle = getComputedStyle(titlebar);
      return {
        x: usable.x,
        y: usable.y,
        elementTag: element?.tagName ?? null,
        elementClass: element?.className?.toString?.() ?? null,
        elementAppRegion: elementStyle?.getPropertyValue("-webkit-app-region") || elementStyle?.webkitAppRegion || null,
        titlebarAppRegion: titlebarStyle.getPropertyValue("-webkit-app-region") || titlebarStyle.webkitAppRegion || null
      };
    })()`
  );
  if (!point) throw new Error("Titlebar center not found");
  return point;
}

async function desktopMaximizedState(client) {
  return evaluate(
    client,
    `window.megleDesktop?.windowControls?.isMaximized?.()`
  );
}

async function waitForMaximizedState(client, expected, timeoutMs = 3500) {
  const deadline = Date.now() + timeoutMs;
  let last = null;
  while (Date.now() < deadline) {
    last = await desktopMaximizedState(client);
    if (last === expected) return { matched: true, value: last };
    await delay(150);
  }
  return { matched: false, value: last };
}

async function titlebarDragEvidence(client) {
  const bridge = await evaluate(
    client,
    `(() => ({
      hasDesktop: Boolean(window.megleDesktop),
      hasWindowControls: Boolean(window.megleDesktop?.windowControls),
      hasIsMaximized: typeof window.megleDesktop?.windowControls?.isMaximized === "function",
      hasMaximize: typeof window.megleDesktop?.windowControls?.maximize === "function"
    }))()`
  );
  if (!bridge.hasIsMaximized || !bridge.hasMaximize) {
    return { bridgeAvailable: false, bridge };
  }

  const point = await titlebarCenterBlankPoint(client);
  return {
    bridgeAvailable: true,
    bridge,
    point,
    nativeDragRegion: point.titlebarAppRegion === "drag"
  };
}

async function searchNoDragEvidence(client) {
  await clickSelector(client, ".shell-titlebar-center .search-bar .overlay-field-input");
  return evaluate(
    client,
    `(() => {
      const input = document.querySelector(".shell-titlebar-center .search-bar .overlay-field-input");
      const style = input ? getComputedStyle(input) : null;
      return {
        searchInputPresent: Boolean(input),
        activeElementTag: document.activeElement?.tagName ?? null,
        activeElementClass: document.activeElement?.className?.toString?.() ?? null,
        focused: document.activeElement === input,
        appRegion: style?.getPropertyValue("-webkit-app-region") || style?.webkitAppRegion || null
      };
    })()`
  );
}

async function pressKey(client, key, code = key, windowsVirtualKeyCode = 0) {
  await client.send("Input.dispatchKeyEvent", {
    type: "keyDown",
    key,
    code,
    windowsVirtualKeyCode
  });
  await client.send("Input.dispatchKeyEvent", {
    type: "keyUp",
    key,
    code,
    windowsVirtualKeyCode
  });
}

async function waitForCompactPopoverState(client, expected, timeoutMs = 5000) {
  const expectedValue = expected === null ? "null" : JSON.stringify(expected);
  return waitFor(
    client,
    `(() => {
      const roots = [...document.querySelectorAll("#notifications-root, #filter-root, #sort-root")];
      if (${expectedValue} === null) return roots.length === 0;
      const rootName = roots[0]?.id.replace(/-root$/, "");
      return roots.length === 1 && rootName === ${expectedValue};
    })()`,
    timeoutMs,
    expected === null ? "compact popovers closed" : `${expected} compact popover`
  );
}

function notificationAnchorGeometryExpression() {
  return `(() => {
    const button = document.querySelector('[data-titlebar-control="notifications"]');
    const popover = document.querySelector('#notifications-root');
    if (!button || !popover || getComputedStyle(popover).visibility !== "visible") return false;
    const buttonRect = button.getBoundingClientRect();
    const popoverRect = popover.getBoundingClientRect();
    return Math.abs(popoverRect.right - buttonRect.right) <= 1 &&
      Math.abs(popoverRect.top - buttonRect.bottom - 6) <= 1 &&
      popoverRect.left >= 0 &&
      popoverRect.top >= 0 &&
      popoverRect.right <= window.innerWidth &&
      popoverRect.bottom <= window.innerHeight;
  })()`;
}

async function waitForNotificationAnchorGeometry(client, label, timeoutMs = 10000) {
  return waitFor(client, notificationAnchorGeometryExpression(), timeoutMs, label);
}

async function notificationMaximizeAnchorEvidence(client) {
  const bridgeAvailable = await evaluate(
    client,
    `typeof window.megleDesktop?.windowControls?.maximize === "function" &&
      typeof window.megleDesktop?.windowControls?.isMaximized === "function"`
  );
  if (!bridgeAvailable) return { bridgeAvailable: false };

  if (await desktopMaximizedState(client)) {
    await evaluate(client, `window.megleDesktop.windowControls.maximize()`);
    await waitForMaximizedState(client, false);
  }
  await clickSelector(client, `[data-compact-popover-trigger="notifications"]`);
  await waitForCompactPopoverState(client, "notifications");
  await waitForNotificationAnchorGeometry(client, "notification anchor before maximize");
  const before = await evaluate(client, compactPopoverEvidenceExpression());

  await evaluate(client, `window.megleDesktop.windowControls.maximize()`);
  const maximizedState = await waitForMaximizedState(client, true);
  await waitForNotificationAnchorGeometry(client, "notification anchor after maximize");
  const maximized = await evaluate(client, compactPopoverEvidenceExpression());

  await evaluate(client, `window.megleDesktop.windowControls.maximize()`);
  const restoredState = await waitForMaximizedState(client, false);
  await waitForNotificationAnchorGeometry(client, "notification anchor after restore");
  const restored = await evaluate(client, compactPopoverEvidenceExpression());
  await pressKey(client, "Escape", "Escape", 27);
  await waitForCompactPopoverState(client, null);
  return { before, bridgeAvailable: true, maximized, maximizedState, restored, restoredState };
}

async function compactPopoverDismissalEvidence(client) {
  const steps = [];

  await pressKey(client, "Escape", "Escape", 27);
  await waitForCompactPopoverState(client, null).catch(() => undefined);

  for (const name of ["notifications", "filter", "sort"]) {
    const triggerSelector = name === "notifications"
      ? `[data-compact-popover-trigger="notifications"]`
      : `[data-titlebar-control="library-${name}"]`;
    await clickSelector(client, triggerSelector);
    await waitForCompactPopoverState(client, name);
    const screenshotPath =
      name === "notifications"
        ? await screenshot(client, "ui-notification-compact-popover.png")
        : null;
    steps.push({
      action: `open-${name}`,
      expected: name,
      screenshotPath,
      evidence: await evaluate(client, compactPopoverEvidenceExpression())
    });
    if (name === "notifications") {
      await clickSelector(client, ".notification-open-center");
      await waitForCompactPopoverState(client, null);
      await waitForTargetCount((page) => page.url.includes("#/notifications"), 1, 10000);
      steps.push({
        action: "open-full-center-closes-notifications",
        expected: null,
        evidence: await evaluate(client, compactPopoverEvidenceExpression())
      });
      const notificationTarget = await waitForTarget(
        (page) => page.url.includes("#/notifications"),
        10000
      );
      const notificationClient = new CdpClient(notificationTarget.webSocketDebuggerUrl);
      await notificationClient.ready;
      await notificationClient.send("Runtime.enable");
      await evaluate(
        notificationClient,
        `(async () => (await window.megleDesktop?.closeNotificationsWindow?.()) ?? false)()`
      );
      notificationClient.close();
      await waitForTargetCount((page) => page.url.includes("#/notifications"), 0, 10000);
    }
  }

  await pressKey(client, "Escape", "Escape", 27);
  await waitForCompactPopoverState(client, null);
  steps.push({
    action: "escape-closes-sort",
    expected: null,
    evidence: await evaluate(client, compactPopoverEvidenceExpression())
  });

  await clickSelector(client, `[data-compact-popover-trigger="notifications"]`);
  await waitForCompactPopoverState(client, "notifications");
  steps.push({
    action: "open-notifications-before-workspace-click",
    expected: "notifications",
    evidence: await evaluate(client, compactPopoverEvidenceExpression())
  });

  await clickSelector(client, ".grid-surface", 0.5, 0.5);
  await waitForCompactPopoverState(client, null);
  steps.push({
    action: "workspace-click-closes-notifications",
    expected: null,
    evidence: await evaluate(client, compactPopoverEvidenceExpression())
  });

  await clickSelector(client, `[data-compact-popover-trigger="notifications"]`);
  await waitForCompactPopoverState(client, "notifications");
  await evaluate(client, `window.dispatchEvent(new Event("blur"))`);
  await waitForCompactPopoverState(client, null);
  steps.push({
    action: "window-blur-closes-notifications",
    expected: null,
    evidence: await evaluate(client, compactPopoverEvidenceExpression())
  });

  return { steps };
}

async function notificationPopoverFixtureEvidence(client) {
  const fixtureUrl = `${webUrl}/tests/fixtures/notification-popover.html?state=full`;
  const opened = await evaluate(
    client,
    `Boolean(window.open(${JSON.stringify(fixtureUrl)}, "megle-notification-popover-fixture", "popup=yes,width=1040,height=760,resizable=yes"))`
  );
  if (!opened) throw new Error("Electron refused to open the notification visual fixture window");

  const target = await waitForTarget(
    (page) => page.url.includes("/tests/fixtures/notification-popover.html"),
    10000
  );
  const fixtureClient = new CdpClient(target.webSocketDebuggerUrl);
  await fixtureClient.ready;
  const fixtureConsoleErrors = [];
  const fixtureNetworkProblems = [];
  fixtureClient.on("Runtime.consoleAPICalled", (event) => {
    if (event.type !== "error") return;
    fixtureConsoleErrors.push({
      type: event.type,
      text: event.args?.map((arg) => arg.value ?? arg.description ?? "").join(" ") ?? ""
    });
  });
  fixtureClient.on("Log.entryAdded", (event) => {
    if (event.entry?.level === "error") {
      fixtureConsoleErrors.push({ type: "log", text: event.entry.text ?? "" });
    }
  });
  fixtureClient.on("Network.responseReceived", (event) => {
    if ((event.response?.status ?? 0) >= 400) {
      fixtureNetworkProblems.push({
        url: event.response?.url ?? "",
        status: event.response?.status ?? 0,
        type: event.type
      });
    }
  });
  fixtureClient.on("Network.loadingFailed", (event) => {
    if (event.type !== "WebSocket") {
      fixtureNetworkProblems.push({
        requestId: event.requestId,
        status: "loadingFailed",
        type: event.type,
        errorText: event.errorText
      });
    }
  });
  await Promise.all([
    fixtureClient.send("Page.enable"),
    fixtureClient.send("Runtime.enable"),
    fixtureClient.send("Network.enable"),
    fixtureClient.send("Log.enable")
  ]);
  await fixtureClient.send("Page.bringToFront");

  async function navigate(state) {
    await fixtureClient.send("Page.navigate", {
      url: `${webUrl}/tests/fixtures/notification-popover.html?state=${state}`
    });
    try {
      await waitFor(
        fixtureClient,
        `Boolean(document.querySelector('[data-notification-fixture="${state}"]') && document.querySelector('#notifications-root'))`,
        10000,
        `notification ${state} visual fixture`
      );
    } catch (error) {
      const diagnosticPath = path.join(logDir, `desktop-ui-regression-notification-${state}-timeout.json`);
      const diagnostic = {
        capturedAtUtc: new Date().toISOString(),
        page: await evaluate(
          fixtureClient,
          `(() => ({
            url: location.href,
            readyState: document.readyState,
            fixtureState: document.querySelector(".notification-visual-fixture")?.getAttribute("data-notification-fixture") ?? null,
            fixtureText: document.querySelector(".notification-visual-fixture")?.textContent?.trim()?.slice(0, 300) ?? null,
            fixtureRoot: Boolean(document.querySelector(".notification-visual-fixture")),
            notificationRoot: Boolean(document.querySelector("#notifications-root")),
            notificationRootAriaHidden: document.querySelector("#notifications-root")?.getAttribute("aria-hidden") ?? null,
            bodyText: document.body?.textContent?.trim()?.slice(0, 500) ?? null
          }))()`
        ),
        consoleErrors: fixtureConsoleErrors,
        networkProblems: fixtureNetworkProblems,
        error: String(error)
      };
      await writeFile(diagnosticPath, JSON.stringify(diagnostic, null, 2));
      throw new Error(`${String(error)}; diagnostic=${diagnosticPath}`);
    }
    await fixtureClient.send("Page.bringToFront");
    await fixtureClient.send("Page.captureScreenshot", {
      captureBeyondViewport: false,
      format: "png"
    });
    await waitForNotificationAnchorGeometry(
      fixtureClient,
      `anchored notification ${state} visual fixture`
    );
    return evaluate(fixtureClient, compactPopoverEvidenceExpression());
  }

  try {
    const full = await navigate("full");
    const fullRendererScreenshot = await rendererScreenshot(
      fixtureClient,
      "ui-notification-compact-fixture-full.png"
    );

    await evaluate(
      fixtureClient,
      `window.notificationPopoverFixture.resetMeasurements()`
    );
    const pointerChurnBefore = await evaluate(
      fixtureClient,
      `window.notificationPopoverFixture.getMetrics()`
    );
    await evaluate(
      fixtureClient,
      `window.notificationPopoverFixture.churnPointerStyles(500)`
    );
    await delay(180);
    const pointerChurnAfter = await evaluate(
      fixtureClient,
      `window.notificationPopoverFixture.getMetrics()`
    );

    const anchorBeforeRelocation = pointerChurnAfter.anchorId;
    await evaluate(
      fixtureClient,
      `window.notificationPopoverFixture.relocate("center")`
    );
    await waitFor(
      fixtureClient,
      `(() => {
        const metrics = window.notificationPopoverFixture?.getMetrics();
        return metrics?.location === "center" && metrics.anchorId !== ${JSON.stringify(anchorBeforeRelocation)};
      })()`,
      10000,
      "notification fixture titlebar relocation"
    );
    await fixtureClient.send("Page.bringToFront");
    await fixtureClient.send("Page.captureScreenshot", {
      captureBeyondViewport: false,
      format: "png"
    });
    await waitForNotificationAnchorGeometry(
      fixtureClient,
      "notification anchor after titlebar relocation"
    );
    const relocated = await evaluate(fixtureClient, compactPopoverEvidenceExpression());

    await evaluate(
      fixtureClient,
      `window.notificationPopoverFixture.resetMeasurements()`
    );
    const detachedChurnBefore = await evaluate(
      fixtureClient,
      `window.notificationPopoverFixture.getMetrics()`
    );
    await evaluate(
      fixtureClient,
      `window.notificationPopoverFixture.churnDetachedPointerStyles(500)`
    );
    await delay(180);
    const detachedChurnAfter = await evaluate(
      fixtureClient,
      `window.notificationPopoverFixture.getMetrics()`
    );

    const anchorBeforeRemount = detachedChurnAfter.anchorId;
    await evaluate(fixtureClient, `window.notificationPopoverFixture.remount()`);
    await waitFor(
      fixtureClient,
      `window.notificationPopoverFixture?.getMetrics().anchorId !== ${JSON.stringify(anchorBeforeRemount)}`,
      10000,
      "notification fixture anchor remount"
    );
    await fixtureClient.send("Page.bringToFront");
    await fixtureClient.send("Page.captureScreenshot", {
      captureBeyondViewport: false,
      format: "png"
    });
    await waitForNotificationAnchorGeometry(
      fixtureClient,
      "notification anchor after remount"
    );
    const remounted = await evaluate(fixtureClient, compactPopoverEvidenceExpression());

    await evaluate(
      fixtureClient,
      `window.notificationPopoverFixture.relocate("right")`
    );
    await fixtureClient.send("Page.bringToFront");
    await fixtureClient.send("Page.captureScreenshot", {
      captureBeyondViewport: false,
      format: "png"
    });
    await waitForNotificationAnchorGeometry(
      fixtureClient,
      "notification anchor after returning right"
    );
    const resize = await resizeScriptOpenedWindow(fixtureClient, 800, 720);
    await waitFor(
      fixtureClient,
      `window.innerWidth <= 820 && window.innerHeight >= 620`,
      10000,
      "narrow notification fixture viewport"
    );
    await fixtureClient.send("Page.bringToFront");
    await fixtureClient.send("Page.captureScreenshot", {
      captureBeyondViewport: false,
      format: "png"
    });
    await waitForNotificationAnchorGeometry(
      fixtureClient,
      "notification anchor after 784px resize"
    );
    const narrow = await evaluate(fixtureClient, compactPopoverEvidenceExpression());
    const narrowRendererScreenshot = await rendererScreenshot(
      fixtureClient,
      "ui-notification-compact-fixture-full-narrow.png"
    );
    await fixtureClient.send("Emulation.setPageScaleFactor", { pageScaleFactor: 1.25 });
    await fixtureClient.send("Page.captureScreenshot", {
      captureBeyondViewport: false,
      format: "png"
    });
    await waitForNotificationAnchorGeometry(
      fixtureClient,
      "notification anchor after page zoom"
    );
    const zoomed = await evaluate(fixtureClient, compactPopoverEvidenceExpression());
    await fixtureClient.send("Emulation.setPageScaleFactor", { pageScaleFactor: 1.5 });
    await fixtureClient.send("Page.captureScreenshot", {
      captureBeyondViewport: false,
      format: "png"
    });
    await waitForNotificationAnchorGeometry(
      fixtureClient,
      "notification anchor after 150 percent page zoom"
    );
    const zoomed150 = await evaluate(fixtureClient, compactPopoverEvidenceExpression());
    const zoomed150RendererScreenshot = await rendererScreenshot(
      fixtureClient,
      "ui-notification-compact-fixture-full-150.png"
    );
    await fixtureClient.send("Emulation.setPageScaleFactor", { pageScaleFactor: 1 });
    await waitForNotificationAnchorGeometry(
      fixtureClient,
      "notification anchor after zoom reset"
    );
    const readPreferenceEvidence = () => evaluate(
      fixtureClient,
      `(() => {
        const root = document.querySelector("#notifications-root");
        const style = root ? getComputedStyle(root) : null;
        const backdrop = root?.querySelector(":scope > .liquid-glass-backdrop");
        const backdropStyle = backdrop ? getComputedStyle(backdrop) : null;
        return {
          reducedMotion: {
            matches: matchMedia("(prefers-reduced-motion: reduce)").matches,
            animationName: style?.animationName ?? null,
            animationDuration: style?.animationDuration ?? null,
            transform: style?.transform ?? null
          },
          reducedTransparency: {
            matches: matchMedia("(prefers-reduced-transparency: reduce)").matches,
            backgroundColor: style?.backgroundColor ?? null,
            backdropFilter: style?.backdropFilter ?? null,
            childBackdropFilter: backdropStyle?.backdropFilter ?? null
          }
        };
      })()`
    );
    await fixtureClient.send("Emulation.setEmulatedMedia", {
      media: "",
      features: [{ name: "prefers-reduced-motion", value: "reduce" }]
    });
    await delay(100);
    const reducedMotion = await readPreferenceEvidence();
    await fixtureClient.send("Emulation.setEmulatedMedia", {
      media: "",
      features: [{ name: "prefers-reduced-transparency", value: "reduce" }]
    });
    await delay(100);
    const reducedTransparency = await readPreferenceEvidence();
    await fixtureClient.send("Emulation.setEmulatedMedia", { media: "", features: [] });
    await delay(100);
    const failure = await navigate("failure");
    const failureScreenshot = await rendererScreenshot(
      fixtureClient,
      "ui-notification-compact-fixture-failure.png"
    );
    const empty = await navigate("empty");
    const emptyScreenshot = await rendererScreenshot(
      fixtureClient,
      "ui-notification-compact-fixture-empty.png"
    );
    const degraded = await navigate("degraded");
    const degradedScreenshot = await rendererScreenshot(
      fixtureClient,
      "ui-notification-compact-fixture-degraded.png"
    );

    return {
      full,
      narrow,
      zoomed,
      zoomed150,
      relocated,
      remounted,
      failure,
      empty,
      degraded,
      preferences: { reducedMotion, reducedTransparency },
      resize,
      measurement: {
        detachedChurnAfter,
        detachedChurnBefore,
        pointerChurnAfter,
        pointerChurnBefore
      },
      screenshots: {
        fullRenderer: fullRendererScreenshot,
        narrowRenderer: narrowRendererScreenshot,
        zoomed150Renderer: zoomed150RendererScreenshot,
        failure: failureScreenshot,
        empty: emptyScreenshot,
        degraded: degradedScreenshot
      },
      navigationDiagnostics: {
        consoleErrors: fixtureConsoleErrors,
        networkProblems: fixtureNetworkProblems
      }
    };
  } finally {
    await evaluate(fixtureClient, `window.close()`).catch(() => undefined);
    fixtureClient.close();
    await waitForTargetCount(
      (page) => page.url.includes("/tests/fixtures/notification-popover.html"),
      0,
      10000
    ).catch(() => undefined);
  }
}

async function notificationWindowFixtureEvidence(client) {
  const fixtureUrl = `${webUrl}/tests/fixtures/notification-window.html?state=full`;
  const opened = await evaluate(
    client,
    `Boolean(window.open(${JSON.stringify(fixtureUrl)}, "megle-notification-window-fixture", "popup=yes,width=1040,height=760,resizable=yes"))`
  );
  if (!opened) throw new Error("Electron refused to open the notification window fixture");

  const target = await waitForTarget(
    (page) => page.url.includes("/tests/fixtures/notification-window.html"),
    10000
  );
  const fixtureClient = new CdpClient(target.webSocketDebuggerUrl);
  await fixtureClient.ready;
  await Promise.all([
    fixtureClient.send("Page.enable"),
    fixtureClient.send("Runtime.enable")
  ]);

  const evidenceExpression = `(() => {
    const owner = document.querySelector("[data-notification-window-fixture]");
    const body = document.querySelector(".notification-window-body");
    const nav = document.querySelector(".notification-window-nav");
    const content = document.querySelector(".notification-window-content");
    const titlebar = document.querySelector(".secondary-window-titlebar");
    const bodyStyle = body ? getComputedStyle(body) : null;
    const navStyle = nav ? getComputedStyle(nav) : null;
    const contentStyle = content ? getComputedStyle(content) : null;
    const titlebarStyle = titlebar ? getComputedStyle(titlebar) : null;
    return {
      state: owner?.getAttribute("data-notification-window-fixture") ?? null,
      viewport: { width: window.innerWidth, height: window.innerHeight },
      selectedPage: document.querySelector('.notification-window-tab[aria-selected="true"]')?.getAttribute("data-fixture-page") ?? null,
      tabCount: document.querySelectorAll('.notification-window-tab[role="tab"]').length,
      tabOrientation: document.querySelector('.notification-window-tabs[role="tablist"]')?.getAttribute("aria-orientation") ?? null,
      narrowQueryMatches: window.matchMedia("(max-width: 760px)").matches,
      bodyGrid: bodyStyle?.gridTemplateColumns ?? null,
      bodyBackground: bodyStyle?.backgroundImage ?? null,
      navBorderRightWidth: navStyle?.borderRightWidth ?? null,
      navBackground: navStyle?.backgroundColor ?? null,
      contentOverflow: contentStyle?.overflow ?? null,
      titlebarBorderBottomWidth: titlebarStyle?.borderBottomWidth ?? null,
      status: {
        active: document.querySelectorAll(".notification-status-operation").length,
        cancelActions: document.querySelectorAll('.notification-status-operation button[aria-label^="取消 "]').length,
        determinate: document.querySelectorAll('.notification-window-progress:not(.indeterminate)').length,
        indeterminate: document.querySelectorAll(".notification-window-progress.indeterminate").length,
        thumbnails: document.querySelectorAll(".notification-thumbnail-card").length,
        results: document.querySelectorAll(".notification-status-result").length,
        degraded: Boolean(document.querySelector(".notification-status-health"))
      },
      history: {
        present: Boolean(document.querySelector(".notification-history-view")),
        filterCount: document.querySelectorAll(".notification-history-filters select").length,
        renderedRows: document.querySelectorAll(".notification-history-row").length,
        actionError: document.querySelector(".notification-row-action-error")?.textContent?.trim() ?? null,
        footer: document.querySelector(".notification-history-footer")?.textContent?.trim() ?? null
      },
      logs: {
        present: Boolean(document.querySelector(".notification-process-log-view")),
        renderedLines: document.querySelectorAll(".notification-log-line").length,
        gap: document.querySelector(".notification-log-gap")?.textContent?.trim() ?? null,
        liveState: document.querySelector(".notification-log-live-state")?.textContent?.trim() ?? null,
        actionMessage: document.querySelector(".notification-log-footer span:last-child")?.textContent?.trim() ?? null,
        controls: {
          pause: Boolean(document.querySelector('button[aria-label="暂停日志"], button[aria-label="继续日志"]')),
          follow: Boolean(document.querySelector('button[aria-label="自动跟随最新日志"]')),
          copy: Boolean(document.querySelector('button[aria-label="复制选中或可见日志"]')),
          clear: Boolean(document.querySelector('button[aria-label="清空当前会话日志"]')),
          export: Boolean(document.querySelector('button[aria-label="导出当前日志"]'))
        }
      },
      metrics: window.notificationWindowFixture?.getMetrics() ?? null
    };
  })()`;

  try {
    await waitFor(
      fixtureClient,
      `Boolean(document.querySelector('[data-notification-window-fixture="full"] .notification-status-view'))`,
      10000,
      "full notification window fixture"
    );
    const status = await evaluate(fixtureClient, evidenceExpression);
    const statusScreenshot = await rendererScreenshot(
      fixtureClient,
      "ui-notification-window-fixture-status.png"
    );

    await clickSelector(fixtureClient, ".notification-thumbnail-failure-action");
    await waitFor(
      fixtureClient,
      `Boolean(document.querySelector(".notification-process-log-view")) && window.notificationWindowFixture?.getMetrics().activeLogSubscriptions === 1`,
      10000,
      "first-entry thumbnail failure logs"
    );
    const firstFailureEvidenceExpression = `(() => {
      const list = document.querySelector(".notification-log-list");
      if (!(list instanceof HTMLElement)) return { visibleActionableRows: 0, visibleFailureRows: 0 };
      const listRect = list.getBoundingClientRect();
      const rows = [...document.querySelectorAll(".notification-log-line")];
      const visibleRows = rows.filter((row) => {
        const rect = row.getBoundingClientRect();
        return rect.bottom > listRect.top && rect.top < listRect.bottom;
      });
      const visibleFailureRows = visibleRows.filter((row) => row.textContent?.includes("failure-"));
      return {
        clientHeight: list.clientHeight,
        scrollHeight: list.scrollHeight,
        scrollTop: list.scrollTop,
        mountedIndices: rows.map((row) => Number(row.getAttribute("data-index"))),
        visibleRows: visibleRows.length,
        visibleFailureRows: visibleFailureRows.length,
        visibleActionableRows: visibleFailureRows.filter((row) => row instanceof HTMLButtonElement && !row.disabled).length
      };
    })()`;
    const failureDeadline = Date.now() + 10000;
    let firstFailureBeforeAction = null;
    while (Date.now() < failureDeadline) {
      firstFailureBeforeAction = await evaluate(fixtureClient, firstFailureEvidenceExpression);
      if (firstFailureBeforeAction?.visibleActionableRows > 0) break;
      await delay(200);
    }
    if (!firstFailureBeforeAction || firstFailureBeforeAction.visibleActionableRows < 1) {
      throw new Error(
        `Timed out waiting for visible actionable failure row on first drilldown; evidence=${JSON.stringify(firstFailureBeforeAction)}`
      );
    }
    await evaluate(
      fixtureClient,
      `(() => {
        const list = document.querySelector(".notification-log-list");
        if (!(list instanceof HTMLElement)) return false;
        const listRect = list.getBoundingClientRect();
        const button = [...document.querySelectorAll("button.notification-log-line.actionable")].find((row) => {
          const rect = row.getBoundingClientRect();
          return row.textContent?.includes("failure-") && rect.bottom > listRect.top && rect.top < listRect.bottom;
        });
        button?.click();
        return Boolean(button);
      })()`
    );
    await waitFor(
      fixtureClient,
      `document.querySelector(".notification-log-footer span:last-child")?.textContent?.includes("媒体")`,
      10000,
      "first-entry failure action"
    );
    const firstFailureDrilldown = {
      ...firstFailureBeforeAction,
      actionMessage: await evaluate(
        fixtureClient,
        `document.querySelector(".notification-log-footer span:last-child")?.textContent?.trim() ?? null`
      )
    };

    await clickSelector(fixtureClient, '[data-fixture-page="history"]');
    await waitFor(
      fixtureClient,
      `Boolean(document.querySelector(".notification-history-view")) && window.notificationWindowFixture?.getMetrics().historyQueries > 0`,
      10000,
      "notification history fixture"
    );
    await delay(120);
    const historyInitial = await evaluate(fixtureClient, evidenceExpression);
    const retrySelector = '[data-notification-operation$=":retry-error"] .notification-row-action';
    if (await evaluate(fixtureClient, `Boolean(document.querySelector(${JSON.stringify(retrySelector)}))`)) {
      await evaluate(
        fixtureClient,
        `document.querySelector(${JSON.stringify(retrySelector)})?.click()`
      );
      await waitFor(
        fixtureClient,
        `Boolean(document.querySelector(".notification-row-action-error"))`,
        10000,
        "notification retry error fixture"
      );
    }
    const loadMoreSelector = ".notification-history-footer .notification-primary-action";
    if (await evaluate(fixtureClient, `Boolean(document.querySelector(${JSON.stringify(loadMoreSelector)}))`)) {
      await clickSelector(fixtureClient, loadMoreSelector);
      await delay(120);
    }
    const history = await evaluate(fixtureClient, evidenceExpression);
    const historyScreenshot = await rendererScreenshot(
      fixtureClient,
      "ui-notification-window-fixture-history.png"
    );

    await clickSelector(fixtureClient, '[data-fixture-page="logs"]');
    await waitFor(
      fixtureClient,
      `Boolean(document.querySelector(".notification-process-log-view")) && window.notificationWindowFixture?.getMetrics().activeLogSubscriptions === 1`,
      10000,
      "notification logs fixture subscription"
    );
    await evaluate(
      fixtureClient,
      `document.querySelector('button[aria-label="暂停日志"]')?.click()`
    );
    await waitFor(
      fixtureClient,
      `Boolean(document.querySelector('button[aria-label="继续日志"]'))`,
      10000,
      "notification paused log control"
    );
    await evaluate(fixtureClient, `window.notificationWindowFixture.emitGap()`);
    await waitFor(
      fixtureClient,
      `document.querySelector(".notification-log-live-state")?.textContent?.includes("条待显示")`,
      10000,
      "notification paused log buffer"
    );
    await evaluate(
      fixtureClient,
      `document.querySelector('button[aria-label="继续日志"]')?.click()`
    );
    await waitFor(
      fixtureClient,
      `Boolean(document.querySelector(".notification-log-gap"))`,
      10000,
      "notification log gap fixture"
    );
    const logsGap = await evaluate(fixtureClient, evidenceExpression);
    await evaluate(
      fixtureClient,
      `document.querySelector('button[aria-label="自动跟随最新日志"]')?.click()`
    );
    await evaluate(
      fixtureClient,
      `document.querySelector('button[aria-label="复制选中或可见日志"]')?.click()`
    );
    await waitFor(
      fixtureClient,
      `Boolean(document.querySelector(".notification-log-footer span:last-child")?.textContent?.trim())`,
      10000,
      "notification log copy fixture"
    );
    await evaluate(
      fixtureClient,
      `document.querySelector('button[aria-label="导出当前日志"]')?.click()`
    );
    await waitFor(
      fixtureClient,
      `document.querySelector(".notification-log-footer span:last-child")?.textContent?.includes("已导出")`,
      10000,
      "notification log export fixture"
    );
    await evaluate(
      fixtureClient,
      `document.querySelector('button[aria-label="清空当前会话日志"]')?.click()`
    );
    await waitFor(
      fixtureClient,
      `document.querySelector(".notification-log-footer span:last-child")?.textContent?.includes("已清空")`,
      10000,
      "notification log clear fixture"
    );
    const logs = await evaluate(fixtureClient, evidenceExpression);
    const logsScreenshot = await rendererScreenshot(
      fixtureClient,
      "ui-notification-window-fixture-logs.png"
    );

    await clickSelector(fixtureClient, '[data-fixture-page="status"]');
    await waitFor(
      fixtureClient,
      `window.notificationWindowFixture?.getMetrics().activeLogSubscriptions === 0 && !document.querySelector(".notification-process-log-view")`,
      10000,
      "notification log tab cleanup fixture"
    );
    const cleanup = await evaluate(fixtureClient, evidenceExpression);

    const resize = await resizeScriptOpenedWindow(fixtureClient, 640, 620);
    await waitFor(fixtureClient, `window.innerWidth <= 660`, 10000, "narrow notification window fixture");
    await evaluate(fixtureClient, `window.dispatchEvent(new Event("resize"))`);
    await waitFor(
      fixtureClient,
      `document.querySelector('.notification-window-tabs[role="tablist"]')?.getAttribute("aria-orientation") === "horizontal"`,
      10000,
      "narrow notification tab orientation"
    );
    const narrow = await evaluate(fixtureClient, evidenceExpression);
    const narrowScreenshot = await rendererScreenshot(
      fixtureClient,
      "ui-notification-window-fixture-narrow.png"
    );

    await fixtureClient.send("Page.navigate", {
      url: `${webUrl}/tests/fixtures/notification-window.html?state=degraded`
    });
    await waitFor(
      fixtureClient,
      `Boolean(document.querySelector('[data-notification-window-fixture="degraded"] .notification-status-health'))`,
      10000,
      "degraded notification window fixture"
    );
    const degraded = await evaluate(fixtureClient, evidenceExpression);
    const degradedScreenshot = await rendererScreenshot(
      fixtureClient,
      "ui-notification-window-fixture-degraded.png"
    );

    return {
      status,
      firstFailureDrilldown,
      historyInitial,
      history,
      logsGap,
      logs,
      cleanup,
      narrow,
      degraded,
      resize,
      screenshots: {
        status: statusScreenshot,
        history: historyScreenshot,
        logs: logsScreenshot,
        narrow: narrowScreenshot,
        degraded: degradedScreenshot
      }
    };
  } finally {
    await evaluate(fixtureClient, `window.close()`).catch(() => undefined);
    fixtureClient.close();
    await waitForTargetCount(
      (page) => page.url.includes("/tests/fixtures/notification-window.html"),
      0,
      10000
    ).catch(() => undefined);
  }
}

async function glassMaterialEvidence(client) {
  await pressKey(client, "Escape", "Escape", 27);
  await waitForCompactPopoverState(client, null).catch(() => undefined);
  await clickSelector(client, `[data-compact-popover-trigger="notifications"]`);
  await waitForCompactPopoverState(client, "notifications");
  const evidence = await evaluate(client, glassMaterialEvidenceExpression());
  await pressKey(client, "Escape", "Escape", 27);
  await waitForCompactPopoverState(client, null);
  return evidence;
}

async function settingsAppearanceMatrix(settingsClient) {
  const groups = {
    side: ["side-blur", "side-opacity", "side-overlay-strength", "side-saturation", "side-stroke-opacity"],
    center: ["center-blur", "center-opacity", "center-overlay-strength", "center-saturation", "center-stroke-opacity"],
    dialog: ["dialog-blur", "dialog-opacity", "dialog-overlay-strength", "dialog-backdrop-dim"]
  };
  const groupVariables = {
    side: ["--glass-side-blur", "--glass-side-fill", "--glass-side-saturation", "--glass-side-stroke"],
    center: ["--glass-center-blur", "--glass-center-fill", "--glass-center-saturation", "--glass-center-stroke"],
    dialog: ["--glass-dialog-blur", "--glass-dialog-fill", "--dialog-blur", "--dialog-backdrop-dim"]
  };
  const sliderIds = Object.values(groups).flat();
  const variableNames = Object.values(groupVariables).flat();
  const readState = () => evaluate(
    settingsClient,
    `(() => {
      const sliders = Object.fromEntries(${JSON.stringify(sliderIds)}.map((id) => {
        const input = document.querySelector("#" + id);
        return [id, input instanceof HTMLInputElement ? {
          value: input.value,
          min: input.getAttribute("min"),
          max: input.getAttribute("max")
        } : null];
      }));
      const root = getComputedStyle(document.documentElement);
      return {
        sliders,
        variables: Object.fromEntries(${JSON.stringify(variableNames)}.map((name) => [
          name,
          root.getPropertyValue(name).trim()
        ]))
      };
    })()`
  );
  const setSlider = async (id, value) => {
    await evaluate(
      settingsClient,
      `(() => {
        const input = document.querySelector(${JSON.stringify(`#${id}`)});
        if (!(input instanceof HTMLInputElement)) return false;
        const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set;
        setter?.call(input, ${JSON.stringify(value)});
        input.dispatchEvent(new Event("input", { bubbles: true }));
        input.dispatchEvent(new Event("change", { bubbles: true }));
        return true;
      })()`
    );
    await delay(120);
    return readState();
  };
  const pickVariables = (state, group) => Object.fromEntries(
    groupVariables[group].map((name) => [name, state.variables[name]])
  );

  const initial = await readState();
  const matrix = {};
  for (const [group, ids] of Object.entries(groups)) {
    matrix[group] = {};
    for (const id of ids) {
      const initialSlider = initial.sliders[id];
      if (!initialSlider) {
        matrix[group][id] = { present: false };
        continue;
      }
      const minState = await setSlider(id, initialSlider.min);
      const defaultState = await setSlider(id, initialSlider.value);
      const maxState = await setSlider(id, initialSlider.max);
      const finalState = await setSlider(id, initialSlider.value);
      matrix[group][id] = {
        present: true,
        default: initialSlider.value,
        min: minState.sliders[id]?.value ?? null,
        max: maxState.sliders[id]?.value ?? null,
        restored: finalState.sliders[id]?.value ?? null,
        variables: {
          min: pickVariables(minState, group),
          default: pickVariables(defaultState, group),
          max: pickVariables(maxState, group)
        }
      };
    }
  }
  return { groups, initial, matrix, final: await readState() };
}

async function settingsInterfaceStyleEvidence(client) {
  const materialBefore = await glassMaterialEvidence(client);
  await clickSelector(client, `[data-titlebar-control="open-settings"]`);
  const target = await waitForTarget((page) => page.url.includes("#/settings"), 10000);
  const settingsClient = new CdpClient(target.webSocketDebuggerUrl);
  await settingsClient.ready;
  await Promise.all([
    settingsClient.send("Page.enable"),
    settingsClient.send("Runtime.enable")
  ]);

  try {
    await waitFor(
      settingsClient,
      `Boolean(document.querySelector(".settings-window"))`,
      10000,
      "Settings secondary window"
    );
    await evaluate(
      settingsClient,
      `(() => {
        const tab = [...document.querySelectorAll('.settings-dialog-nav-item')]
          .find((node) => node.textContent?.trim() === '外观');
        if (!(tab instanceof HTMLButtonElement)) return false;
        tab.click();
        return true;
      })()`
    );
    await waitFor(
      settingsClient,
      `Boolean(document.querySelector('#side-blur'))`,
      10000,
      "Settings appearance controls"
    );
    const screenshotPath = await screenshot(settingsClient, "ui-settings-interface-style.png");
    const evidenceBefore = await evaluate(settingsClient, settingsInterfaceStyleEvidenceExpression());
    const appearanceMatrix = await settingsAppearanceMatrix(settingsClient);
    const pointSettingsSlider = await moveToSelector(
      settingsClient,
      '[data-settings-slider-control="side-blur"]',
      0.27,
      0.5
    );
    const pointerSettingsSlider = await evaluate(settingsClient, pointerEvidenceExpression());
    const beforeSideBlur = evidenceBefore.sideBlur;
    const sliderChange = await evaluate(
      settingsClient,
      `(() => {
        const input = document.querySelector("#side-blur");
        if (!(input instanceof HTMLInputElement)) return { changed: false, reason: "side-blur slider missing" };
        const before = input.value;
        const min = input.getAttribute("min") ?? "0";
        const max = input.getAttribute("max") ?? "2";
        const next = before === max ? min : max;
        const valueSetter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set;
        valueSetter?.call(input, next);
        input.dispatchEvent(new Event("input", { bubbles: true }));
        input.dispatchEvent(new Event("change", { bubbles: true }));
        return { changed: input.value !== before, before, after: input.value };
      })()`
    );
    await waitFor(
      client,
      `getComputedStyle(document.documentElement).getPropertyValue("--glass-side-blur").trim() !== ${JSON.stringify(beforeSideBlur)}`,
      5000,
      "main-window side shell blur sync"
    );
    const evidenceAfter = await evaluate(settingsClient, settingsInterfaceStyleEvidenceExpression());
    const materialAfter = await glassMaterialEvidence(client);
    await evaluate(
      settingsClient,
      `(() => {
        const reset = [...document.querySelectorAll(".settings-dialog-content button")]
          .find((button) => button.textContent?.trim() === "恢复默认外观");
        reset?.click();
        return Boolean(reset);
      })()`
    );
    await delay(350);
    fireRendererCommand(settingsClient, `void window.megleDesktop?.closeSettingsWindow?.()`);
    await waitForTargetCount((page) => page.url.includes("#/settings"), 0, 10000);
    return {
      screenshotPath,
      evidence: {
        ...evidenceAfter,
        before: evidenceBefore,
        after: evidenceAfter,
        pointerPoint: pointSettingsSlider,
        pointerSlider: pointerSettingsSlider,
        sliderChange,
        materialBefore,
        materialAfter,
        appearanceMatrix,
        standaloneWindow: true
      }
    };
  } finally {
    fireRendererCommand(settingsClient, `void window.megleDesktop?.closeSettingsWindow?.()`);
    settingsClient.close();
  }
}

async function pluginsSecondaryWindowEvidence(client) {
  const mainBefore = await evaluate(
    client,
    `(() => {
      const controlState = (id) => {
        const element = document.querySelector('[data-titlebar-control="' + id + '"]');
        if (!(element instanceof HTMLElement)) return null;
        const style = getComputedStyle(element);
        const before = getComputedStyle(element, "::before");
        const after = getComputedStyle(element, "::after");
        return {
          borderWidth: style.borderWidth,
          backgroundColor: style.backgroundColor,
          pointerTarget: element.getAttribute("data-interactive-pointer-target"),
          beforeBackgroundImage: before.backgroundImage,
          afterBorderWidth: after.borderWidth,
          afterBoxShadow: after.boxShadow,
          afterBackgroundImage: after.backgroundImage
        };
      };
      return {
        libraryVisible: Boolean(document.querySelector(".grid-surface")),
        pluginsInMain: Boolean(document.querySelector(".plugins-body")),
        oldPrimaryNavPresent: Boolean(document.querySelector(".shell-primary-nav")),
        controlOrder: [...document.querySelectorAll(".shell-titlebar-primary-actions [data-titlebar-control]")]
          .map((node) => node.getAttribute("data-titlebar-control")),
        utilityOrder: [...document.querySelectorAll(".shell-titlebar-utility-group [data-titlebar-control]")]
          .map((node) => node.getAttribute("data-titlebar-control")),
        utilityRightGap: (() => {
          const titlebar = document.querySelector(".shell-titlebar-left")?.getBoundingClientRect();
          const utilities = document.querySelector(".shell-titlebar-utility-group")?.getBoundingClientRect();
          return titlebar && utilities ? titlebar.right - utilities.right : null;
        })(),
        libraryWidth: document.querySelector('[data-titlebar-control="library-switcher"]')
          ?.getBoundingClientRect().width ?? null,
        treeTopGap: (() => {
          const sidebar = document.querySelector(".library-sidebar")?.getBoundingClientRect();
          const firstRow = document.querySelector(".tree-list > .tree-branch > .tree-item")?.getBoundingClientRect();
          return sidebar && firstRow ? firstRow.top - sidebar.top : null;
        })(),
        controls: Object.fromEntries(
          ["library-switcher", "open-plugins", "open-settings", "locate-folder", "toggle-sidebars"]
            .map((id) => [id, controlState(id)])
        )
      };
    })()`
  );

  await clickSelector(client, `[data-titlebar-control="open-plugins"]`);
  const pluginTarget = await waitForTarget((page) => page.url.includes("#/plugins"), 10000);
  const pluginsClient = new CdpClient(pluginTarget.webSocketDebuggerUrl);
  await pluginsClient.ready;
  await Promise.all([
    pluginsClient.send("Page.enable"),
    pluginsClient.send("Runtime.enable")
  ]);

  let settingsClient;
  try {
    await waitFor(pluginsClient, `Boolean(document.querySelector(".plugins-body"))`, 10000, "Plugins secondary window");
    await waitFor(
      pluginsClient,
      `document.querySelector(".secondary-window")?.getAttribute("data-secondary-content-visible") === "true"`,
      10000,
      "Plugins native-window reveal"
    );
    const screenshotPath = await screenshot(pluginsClient, "ui-plugins-secondary-window.png");
    const firstTargets = await waitForTargetCount((page) => page.url.includes("#/plugins"), 1);

    await clickSelector(client, `[data-titlebar-control="open-plugins"]`);
    const repeatedTargets = await waitForTargetCount((page) => page.url.includes("#/plugins"), 1);
    await waitFor(pluginsClient, `document.hasFocus()`, 5000, "existing Plugins window focus");

    await clickSelector(client, `[data-titlebar-control="open-settings"]`);
    const settingsTarget = await waitForTarget((page) => page.url.includes("#/settings"), 10000);
    settingsClient = new CdpClient(settingsTarget.webSocketDebuggerUrl);
    await settingsClient.ready;
    await settingsClient.send("Runtime.enable");
    await waitFor(settingsClient, `Boolean(document.querySelector(".settings-window"))`, 10000, "Settings alongside Plugins");
    await waitFor(
      settingsClient,
      `document.querySelector(".secondary-window")?.getAttribute("data-secondary-content-visible") === "true"`,
      10000,
      "Settings native-window reveal"
    );

    const coexistence = {
      plugins: (await matchingTargets((page) => page.url.includes("#/plugins"))).length,
      settings: (await matchingTargets((page) => page.url.includes("#/settings"))).length
    };
    const mainAfter = await evaluate(
      client,
      `({
        libraryVisible: Boolean(document.querySelector(".grid-surface")),
        pluginsInMain: Boolean(document.querySelector(".plugins-body"))
      })`
    );
    const pluginWindow = await evaluate(
      pluginsClient,
      `(() => {
        const owner = document.querySelector(".secondary-window");
        const backdrop = owner?.querySelector(":scope > .liquid-glass-backdrop");
        const root = getComputedStyle(document.documentElement);
        return {
          role: window.megleDesktop?.windowRole,
          title: document.querySelector(".secondary-window-title")?.textContent?.trim(),
          closePresent: Boolean(document.querySelector(".secondary-window-close")),
          pluginSurfacePresent: Boolean(document.querySelector(".plugins-layout")),
          rendererReady: owner?.getAttribute("data-secondary-renderer-ready"),
          windowVisible: owner?.getAttribute("data-secondary-window-visible"),
          contentVisible: owner?.getAttribute("data-secondary-content-visible"),
          nativeWarmupStartedAt: owner?.getAttribute("data-secondary-native-warmup-started-at"),
          nativeWindowRevealedAt: owner?.getAttribute("data-secondary-native-window-revealed-at"),
          contentVisibleAt: owner?.getAttribute("data-secondary-content-visible-at"),
          revealFallback: owner?.getAttribute("data-secondary-reveal-fallback"),
          material: {
            background: backdrop ? getComputedStyle(backdrop).backgroundColor : null,
            backdropFilter: backdrop ? getComputedStyle(backdrop).backdropFilter : null,
            sideBlur: root.getPropertyValue("--glass-side-blur").trim(),
            sideFill: root.getPropertyValue("--glass-side-fill").trim(),
            sideSaturation: root.getPropertyValue("--glass-side-saturation").trim(),
            sideStroke: root.getPropertyValue("--glass-side-stroke").trim()
          }
        };
      })()`
    );
    const settingsWindow = await evaluate(
      settingsClient,
      `(() => {
        const owner = document.querySelector(".secondary-window");
        const backdrop = owner?.querySelector(":scope > .liquid-glass-backdrop");
        const root = getComputedStyle(document.documentElement);
        return {
          rendererReady: owner?.getAttribute("data-secondary-renderer-ready"),
          windowVisible: owner?.getAttribute("data-secondary-window-visible"),
          contentVisible: owner?.getAttribute("data-secondary-content-visible"),
          nativeWarmupStartedAt: owner?.getAttribute("data-secondary-native-warmup-started-at"),
          nativeWindowRevealedAt: owner?.getAttribute("data-secondary-native-window-revealed-at"),
          contentVisibleAt: owner?.getAttribute("data-secondary-content-visible-at"),
          revealFallback: owner?.getAttribute("data-secondary-reveal-fallback"),
          material: {
            background: backdrop ? getComputedStyle(backdrop).backgroundColor : null,
            backdropFilter: backdrop ? getComputedStyle(backdrop).backdropFilter : null,
            sideBlur: root.getPropertyValue("--glass-side-blur").trim(),
            sideFill: root.getPropertyValue("--glass-side-fill").trim(),
            sideSaturation: root.getPropertyValue("--glass-side-saturation").trim(),
            sideStroke: root.getPropertyValue("--glass-side-stroke").trim()
          }
        };
      })()`
    );

    fireRendererCommand(settingsClient, `void window.megleDesktop?.closeSettingsWindow?.()`);
    await waitForTargetCount((page) => page.url.includes("#/settings"), 0, 10000);
    fireRendererCommand(pluginsClient, `void window.megleDesktop?.closePluginsWindow?.()`);
    await waitForTargetCount((page) => page.url.includes("#/plugins"), 0, 10000);

    return {
      screenshotPath,
      mainBefore,
      mainAfter,
      pluginWindow,
      settingsWindow,
      firstTargetId: firstTargets[0]?.id ?? null,
      repeatedTargetId: repeatedTargets[0]?.id ?? null,
      coexistence
    };
  } finally {
    if (settingsClient) fireRendererCommand(settingsClient, `void window.megleDesktop?.closeSettingsWindow?.()`);
    fireRendererCommand(pluginsClient, `void window.megleDesktop?.closePluginsWindow?.()`);
    settingsClient?.close();
    pluginsClient.close();
  }
}

async function notificationsSecondaryWindowEvidence(client) {
  const open = () =>
    evaluate(
      client,
      `(async () => (await window.megleDesktop?.openNotificationsWindow?.()) ?? false)()`
    );
  await open();
  const firstTarget = await waitForTarget((page) => page.url.includes("#/notifications"), 10000);
  const notificationsClient = new CdpClient(firstTarget.webSocketDebuggerUrl);
  await notificationsClient.ready;
  await Promise.all([
    notificationsClient.send("Page.enable"),
    notificationsClient.send("Runtime.enable")
  ]);

  let reopenedClient;
  try {
    await waitFor(
      notificationsClient,
      `Boolean(document.querySelector(".notification-window"))`,
      10000,
      "Notification secondary window"
    );
    await waitFor(
      notificationsClient,
      `document.querySelector(".secondary-window")?.getAttribute("data-secondary-content-visible") === "true"`,
      10000,
      "Notification native-window reveal"
    );
    await waitFor(
      notificationsClient,
      `Boolean(document.querySelector("#notification-panel-status .notification-status-view"))`,
      10000,
      "Notification default status page"
    );
    const screenshotPath = await screenshot(
      notificationsClient,
      "ui-notifications-secondary-window.png"
    );
    const statusEvidence = await evaluate(
      notificationsClient,
      `(() => {
        const body = document.querySelector(".notification-window-body");
        const nav = document.querySelector(".notification-window-nav");
        const content = document.querySelector(".notification-window-content");
        const titlebar = document.querySelector(".secondary-window-titlebar");
        const bodyStyle = body ? getComputedStyle(body) : null;
        const navStyle = nav ? getComputedStyle(nav) : null;
        const contentStyle = content ? getComputedStyle(content) : null;
        const titlebarStyle = titlebar ? getComputedStyle(titlebar) : null;
        return {
          tabCount: document.querySelectorAll('.notification-window-tab[role="tab"]').length,
          selectedTab: document.querySelector('.notification-window-tab[aria-selected="true"]')?.id ?? null,
          activeCount: document.querySelectorAll(".notification-status-operation").length,
          thumbnailCount: document.querySelectorAll(".notification-thumbnail-card").length,
          resultCount: document.querySelectorAll(".notification-status-result").length,
          degraded: Boolean(document.querySelector(".notification-status-health")),
          bodyGrid: bodyStyle?.gridTemplateColumns ?? null,
          bodyBackground: bodyStyle?.backgroundImage ?? null,
          navBorderRight: navStyle?.borderRight ?? null,
          navBackground: navStyle?.backgroundColor ?? null,
          contentOverflow: contentStyle?.overflow ?? null,
          titlebarBorderBottom: titlebarStyle?.borderBottom ?? null
        };
      })()`
    );

    await clickSelector(notificationsClient, "#notification-tab-history");
    await waitFor(
      notificationsClient,
      `Boolean(document.querySelector(".notification-history-view"))`,
      10000,
      "Notification history page"
    );
    const historyEvidence = await evaluate(
      notificationsClient,
      `(() => ({
        filterCount: document.querySelectorAll(".notification-history-filters select").length,
        virtualRows: document.querySelectorAll(".notification-history-row").length,
        actionCount: document.querySelectorAll(".notification-row-action").length,
        unreadControls: [...document.querySelectorAll("button, input, select")].filter((node) => /未读|已读/.test(node.textContent || node.getAttribute("aria-label") || "")).length
      }))()`
    );

    await clickSelector(notificationsClient, "#notification-tab-logs");
    await waitFor(
      notificationsClient,
      `Boolean(document.querySelector(".notification-process-log-view"))`,
      10000,
      "Notification process log page"
    );
    const logsScreenshotPath = await screenshot(
      notificationsClient,
      "ui-notifications-secondary-window-logs.png"
    );
    const logsEvidence = await evaluate(
      notificationsClient,
      `(() => ({
        sourceFilter: Boolean(document.querySelector('.notification-log-toolbar select option[value="main"]')),
        levelFilter: Boolean(document.querySelector('.notification-log-toolbar select option[value="error"]')),
        pause: Boolean(document.querySelector('button[aria-label="暂停日志"], button[aria-label="继续日志"]')),
        follow: Boolean(document.querySelector('button[aria-label="自动跟随最新日志"]')),
        copy: Boolean(document.querySelector('button[aria-label="复制选中或可见日志"]')),
        clear: Boolean(document.querySelector('button[aria-label="清空当前会话日志"]')),
        export: Boolean(document.querySelector('button[aria-label="导出当前日志"]')),
        virtualLines: document.querySelectorAll(".notification-log-line").length
      }))()`
    );
    await clickSelector(notificationsClient, "#notification-tab-status");
    await waitFor(
      notificationsClient,
      `Boolean(document.querySelector("#notification-panel-status .notification-status-view")) && !document.querySelector(".notification-process-log-view")`,
      10000,
      "Notification log page cleanup"
    );
    const firstTargets = await waitForTargetCount(
      (page) => page.url.includes("#/notifications"),
      1
    );

    await open();
    const repeatedTargets = await waitForTargetCount(
      (page) => page.url.includes("#/notifications"),
      1
    );
    await waitFor(notificationsClient, `document.hasFocus()`, 5000, "existing Notification window focus");

    const initialBounds = await evaluate(
      notificationsClient,
      `(async () => (await window.megleDesktop?.windowControls?.getBounds?.()) ?? null)()`
    );
    const requestedPosition = initialBounds
      ? { x: initialBounds.x + 18, y: initialBounds.y + 18 }
      : null;
    if (requestedPosition) {
      await evaluate(
        notificationsClient,
        `(async () => (await window.megleDesktop?.windowControls?.setPosition?.(${requestedPosition.x}, ${requestedPosition.y})) ?? false)()`
      );
      await delay(250);
    }
    const movedBounds = await evaluate(
      notificationsClient,
      `(async () => (await window.megleDesktop?.windowControls?.getBounds?.()) ?? null)()`
    );
    const windowEvidence = await evaluate(
      notificationsClient,
      `(() => {
        const owner = document.querySelector(".secondary-window");
        const backdrop = owner?.querySelector(":scope > .liquid-glass-backdrop");
        const root = getComputedStyle(document.documentElement);
        return {
          role: window.megleDesktop?.windowRole,
          title: document.querySelector(".secondary-window-title")?.textContent?.trim(),
          closePresent: Boolean(document.querySelector(".secondary-window-close")),
          rendererReady: owner?.getAttribute("data-secondary-renderer-ready"),
          windowVisible: owner?.getAttribute("data-secondary-window-visible"),
          contentVisible: owner?.getAttribute("data-secondary-content-visible"),
          nativeWarmupStartedAt: owner?.getAttribute("data-secondary-native-warmup-started-at"),
          nativeWindowRevealedAt: owner?.getAttribute("data-secondary-native-window-revealed-at"),
          contentVisibleAt: owner?.getAttribute("data-secondary-content-visible-at"),
          revealFallback: owner?.getAttribute("data-secondary-reveal-fallback"),
          material: {
            background: backdrop ? getComputedStyle(backdrop).backgroundColor : null,
            backdropFilter: backdrop ? getComputedStyle(backdrop).backdropFilter : null,
            sideBlur: root.getPropertyValue("--glass-side-blur").trim(),
            sideFill: root.getPropertyValue("--glass-side-fill").trim(),
            sideSaturation: root.getPropertyValue("--glass-side-saturation").trim(),
            sideStroke: root.getPropertyValue("--glass-side-stroke").trim()
          }
        };
      })()`
    );

    fireRendererCommand(
      notificationsClient,
      `void window.megleDesktop?.closeNotificationsWindow?.()`
    );
    await waitForTargetCount((page) => page.url.includes("#/notifications"), 0, 10000);
    notificationsClient.close();

    await open();
    const reopenedTarget = await waitForTarget(
      (page) => page.url.includes("#/notifications"),
      10000
    );
    reopenedClient = new CdpClient(reopenedTarget.webSocketDebuggerUrl);
    await reopenedClient.ready;
    await reopenedClient.send("Runtime.enable");
    await waitFor(
      reopenedClient,
      `document.querySelector(".secondary-window")?.getAttribute("data-secondary-content-visible") === "true"`,
      10000,
      "reopened Notification native-window reveal"
    );
    const restoredBounds = await evaluate(
      reopenedClient,
      `(async () => (await window.megleDesktop?.windowControls?.getBounds?.()) ?? null)()`
    );
    fireRendererCommand(
      reopenedClient,
      `void window.megleDesktop?.closeNotificationsWindow?.()`
    );
    await waitForTargetCount((page) => page.url.includes("#/notifications"), 0, 10000);

    return {
      screenshotPath,
      logsScreenshotPath,
      firstTargetId: firstTargets[0]?.id ?? null,
      repeatedTargetId: repeatedTargets[0]?.id ?? null,
      initialBounds,
      requestedPosition,
      movedBounds,
      restoredBounds,
      pages: { status: statusEvidence, history: historyEvidence, logs: logsEvidence },
      window: windowEvidence
    };
  } finally {
    fireRendererCommand(
      reopenedClient ?? notificationsClient,
      `void window.megleDesktop?.closeNotificationsWindow?.()`
    );
    reopenedClient?.close();
    notificationsClient.close();
  }
}

async function imagePointRatioAt(client, point) {
  return evaluate(
    client,
    `(() => {
      const image = document.querySelector(".central-preview-stage .preview-image");
      if (!image) return null;
      const rect = image.getBoundingClientRect();
      return {
        x: (${point.x} - rect.left) / rect.width,
        y: (${point.y} - rect.top) / rect.height
      };
    })()`
  );
}

async function closeCentralPreview(client) {
  await pressKey(client, "Escape", "Escape", 27);
  await delay(250);
}

function layoutEvidenceExpression() {
  return `(() => {
    const css = (selector) => {
      const element = document.querySelector(selector);
      return element ? getComputedStyle(element) : null;
    };
    const box = (selector) => {
      const element = document.querySelector(selector);
      if (!element) return null;
      const rect = element.getBoundingClientRect();
      return serializeRect(rect);
    };
    const styleState = (selector, pseudo = null) => {
      const element = document.querySelector(selector);
      const style = element ? getComputedStyle(element, pseudo) : null;
      const backdrop = pseudo ? null : element?.querySelector(":scope > .liquid-glass-backdrop") ?? null;
      const backdropStyle = backdrop ? getComputedStyle(backdrop) : null;
      return style
        ? {
            position: style.position,
            display: style.display,
            gridArea: style.gridArea,
            dataLiquidGlass: pseudo ? null : element?.getAttribute("data-liquid-glass") ?? null,
            backgroundColor: style.backgroundColor,
            backgroundImage: style.backgroundImage,
            backdropFilter: style.backdropFilter,
            webkitBackdropFilter: style.webkitBackdropFilter,
            backdropLayerPresent: pseudo ? false : Boolean(backdrop),
            backdropLayerBackdropFilter: backdropStyle?.backdropFilter ?? null,
            backdropLayerWebkitBackdropFilter: backdropStyle?.webkitBackdropFilter ?? null,
            boxShadow: style.boxShadow,
            borderRadius: style.borderRadius,
            borderTop: style.borderTop,
            borderRight: style.borderRight,
            borderBottom: style.borderBottom,
            borderLeft: style.borderLeft,
            borderTopColor: style.borderTopColor,
            borderBottomColor: style.borderBottomColor,
            borderTopWidth: style.borderTopWidth,
            borderBottomWidth: style.borderBottomWidth
          }
        : null;
    };
    const shell = css(".app-shell");
    const html = getComputedStyle(document.documentElement);
    const body = getComputedStyle(document.body);
    const root = css("#root");
    return {
      viewport: { width: window.innerWidth, height: window.innerHeight },
      htmlBackground: html.backgroundColor,
      bodyBackground: body.backgroundColor,
      rootBackground: root?.backgroundColor ?? null,
      shellBackground: shell?.backgroundColor ?? null,
      shellBackgroundImage: shell?.backgroundImage ?? null,
      htmlBackdropFilter: html.backdropFilter,
      bodyBackdropFilter: body.backdropFilter,
      rootBackdropFilter: root?.backdropFilter ?? null,
      shellBackdropFilter: shell?.backdropFilter ?? null,
      titlebarLeft: box(".shell-titlebar-left"),
      titlebarCenter: box(".shell-titlebar-center"),
      titlebarRight: box(".shell-titlebar-right"),
      titlebarLeftStyle: styleState(".shell-titlebar-left"),
      titlebarCenterStyle: styleState(".shell-titlebar-center"),
      titlebarRightStyle: styleState(".shell-titlebar-right"),
      workspace: box(".workspace"),
      gridSurface: box(".grid-surface"),
      gridSurfaceStyle: styleState(".grid-surface"),
      gridSurfaceAfterStyle: styleState(".grid-surface", "::after"),
      inspector: box(".inspector-panel"),
      inspectorStyle: styleState(".inspector-panel"),
      sidebar: box(".library-sidebar"),
      sidebarStyle: styleState(".library-sidebar"),
      joinGaps: {
        left: Math.abs((box(".library-sidebar")?.top ?? 0) - (box(".shell-titlebar-left")?.bottom ?? 0)),
        center: Math.abs((box(".grid-surface")?.top ?? 0) - (box(".shell-titlebar-center")?.bottom ?? 0)),
        right: Math.abs((box(".inspector-panel")?.top ?? 0) - (box(".shell-titlebar-right")?.bottom ?? 0))
      }
    };
    function serializeRect(rect) {
      return { x: rect.x, y: rect.y, width: rect.width, height: rect.height, top: rect.top, right: rect.right, bottom: rect.bottom, left: rect.left };
    }
  })()`;
}

function inspectorPreviewEvidenceExpression() {
  return `(() => {
    const stage = document.querySelector(".inspector-panel .preview-stage");
    const ready = document.querySelector(".inspector-panel .preview-placeholder.ready");
    const image = document.querySelector(".inspector-panel .preview-stage .preview-image");
    const source = document.querySelector(".inspector-panel .preview-placeholder.ready")?.getAttribute("data-preview-source") ?? null;
    const stageStyle = stage ? getComputedStyle(stage) : null;
    const readyStyle = ready ? getComputedStyle(ready) : null;
    const imageStyle = image ? getComputedStyle(image) : null;
    const imageRect = image ? image.getBoundingClientRect() : null;
    const stageRect = stage ? stage.getBoundingClientRect() : null;
    return {
      centralOpen: Boolean(document.querySelector(".central-preview-stage")),
      headingPresent: Boolean(document.querySelector(".inspector-panel .preview-panel-heading")),
      source,
      stageBackground: stageStyle?.backgroundColor ?? null,
      stageBorder: stageStyle?.border ?? null,
      stageBorderWidth: stageStyle?.borderWidth ?? null,
      stageBorderRadius: stageStyle?.borderRadius ?? null,
      stagePadding: stageStyle?.padding ?? null,
      stageBoxShadow: stageStyle?.boxShadow ?? null,
      readyBackground: readyStyle?.backgroundColor ?? null,
      imagePresent: Boolean(image),
      imageTag: image?.tagName ?? null,
      imageObjectFit: imageStyle?.objectFit ?? null,
      imageBorderRadius: imageStyle?.borderRadius ?? null,
      imageNatural: image ? { width: image.naturalWidth, height: image.naturalHeight } : null,
      imageRect: imageRect ? serializeRect(imageRect) : null,
      stageRect: stageRect ? serializeRect(stageRect) : null,
      stageHeight: stageRect?.height ?? null,
      imageCenterDelta: imageRect && stageRect
        ? {
            x: Math.abs((imageRect.left + imageRect.width / 2) - (stageRect.left + stageRect.width / 2)),
            y: Math.abs((imageRect.top + imageRect.height / 2) - (stageRect.top + stageRect.height / 2))
          }
        : null,
      imageWithinStage: imageRect && stageRect
        ? rectWithin(imageRect, stageRect, 2)
        : false
    };
    function serializeRect(rect) {
      return { x: rect.x, y: rect.y, width: rect.width, height: rect.height, top: rect.top, right: rect.right, bottom: rect.bottom, left: rect.left };
    }
    function rectWithin(inner, outer, tolerance) {
      return inner.left >= outer.left - tolerance &&
        inner.right <= outer.right + tolerance &&
        inner.top >= outer.top - tolerance &&
        inner.bottom <= outer.bottom + tolerance;
    }
  })()`;
}

function centralPreviewEvidenceExpression() {
  return `(() => {
    const stage = document.querySelector(".central-preview-stage");
    const viewport = document.querySelector(".central-preview-viewport");
    const transform = document.querySelector(".central-preview-transform");
    const ready = document.querySelector(".central-preview-stage .preview-placeholder.ready");
    const image = document.querySelector(".central-preview-stage .preview-image");
    const titlebar = document.querySelector(".shell-titlebar-center");
    const title = document.querySelector(".titlebar-preview-title");
    const stageStyle = stage ? getComputedStyle(stage) : null;
    const transformStyle = transform ? getComputedStyle(transform) : null;
    const readyStyle = ready ? getComputedStyle(ready) : null;
    const imageStyle = image ? getComputedStyle(image) : null;
    const stageRect = stage ? stage.getBoundingClientRect() : null;
    const transformRect = transform ? transform.getBoundingClientRect() : null;
    const readyRect = ready ? ready.getBoundingClientRect() : null;
    const imageRect = image ? image.getBoundingClientRect() : null;
    const titlebarRect = titlebar ? titlebar.getBoundingClientRect() : null;
    const titleRect = title ? title.getBoundingClientRect() : null;
    const natural = image
      ? image instanceof HTMLCanvasElement
        ? { width: image.width, height: image.height }
        : { width: image.naturalWidth, height: image.naturalHeight }
      : null;
    const layoutSize = image ? { width: image.offsetWidth, height: image.offsetHeight } : null;
    const transformMatrix = transformStyle?.transform && transformStyle.transform !== "none"
      ? new DOMMatrixReadOnly(transformStyle.transform)
      : null;
    const expectedFitScale = stageRect && layoutSize
      ? Math.min(stageRect.width / layoutSize.width, stageRect.height / layoutSize.height)
      : null;
    const actualScale = transformMatrix?.a ?? null;
    return {
      mode: viewport?.getAttribute("data-preview-mode") ?? null,
      transform: transformStyle?.transform ?? null,
      stageBackground: stageStyle?.backgroundColor ?? null,
      stageBorder: stageStyle?.border ?? null,
      stageBorderWidth: stageStyle?.borderWidth ?? null,
      stagePadding: stageStyle?.padding ?? null,
      stageBoxShadow: stageStyle?.boxShadow ?? null,
      stageOutlineStyle: stageStyle?.outlineStyle ?? null,
      stageOutlineWidth: stageStyle?.outlineWidth ?? null,
      readyBackground: readyStyle?.backgroundColor ?? null,
      readyPadding: readyStyle?.padding ?? null,
      imageObjectFit: imageStyle?.objectFit ?? null,
      stageRect: stageRect ? serializeRect(stageRect) : null,
      transformRect: transformRect ? serializeRect(transformRect) : null,
      readyRect: readyRect ? serializeRect(readyRect) : null,
      imageRect: imageRect ? serializeRect(imageRect) : null,
      titleText: title?.textContent?.trim() ?? null,
      titlebarRect: titlebarRect ? serializeRect(titlebarRect) : null,
      titleRect: titleRect ? serializeRect(titleRect) : null,
      titleCenterDelta: titlebarRect && titleRect
        ? Math.abs((titleRect.left + titleRect.width / 2) - (titlebarRect.left + titlebarRect.width / 2))
        : null,
      imageNatural: natural,
      imageLayout: layoutSize,
      expectedFitScale,
      actualScale,
      imageWithinStage: imageRect && stageRect ? rectWithin(imageRect, stageRect, 2) : false,
      inspectorPreviewVisible: Boolean(document.querySelector(".inspector-panel .preview-stage"))
    };
    function serializeRect(rect) {
      return { x: rect.x, y: rect.y, width: rect.width, height: rect.height, top: rect.top, right: rect.right, bottom: rect.bottom, left: rect.left };
    }
    function rectWithin(inner, outer, tolerance) {
      return inner.left >= outer.left - tolerance &&
        inner.right <= outer.right + tolerance &&
        inner.top >= outer.top - tolerance &&
        inner.bottom <= outer.bottom + tolerance;
    }
  })()`;
}

function settingsInterfaceStyleEvidenceExpression() {
  return `(() => {
    const section = document.querySelector(".settings-dialog-content");
    const rangeIds = [
      "window-corner-radius",
      "surface-corner-radius",
      "control-corner-radius",
      "content-corner-radius",
      "side-blur",
      "side-opacity",
      "side-overlay-strength",
      "side-saturation",
      "side-stroke-opacity",
      "center-blur",
      "center-opacity",
      "center-overlay-strength",
      "center-saturation",
      "center-stroke-opacity",
      "dither-opacity",
      "backdrop-gradient-strength",
      "edge-highlight-brightness",
      "halo-brightness",
      "pointer-response-radius",
      "pointer-sensing-distance",
      "pointer-falloff",
      "dialog-blur",
      "dialog-opacity",
      "dialog-overlay-strength",
      "dialog-backdrop-dim"
    ];
    const colorIds = [
      "side-overlay-color",
      "center-overlay-color"
    ];
    const sliders = Object.fromEntries(rangeIds.map((id) => {
      const input = section?.querySelector("#" + CSS.escape(id));
      return [id, {
        present: Boolean(input),
        type: input?.getAttribute("type") ?? null,
        value: input?.value ?? null,
        min: input?.getAttribute("min") ?? null,
        max: input?.getAttribute("max") ?? null,
        step: input?.getAttribute("step") ?? null
      }];
    }));
    const colors = Object.fromEntries(colorIds.map((id) => {
      const input = section?.querySelector("#" + CSS.escape(id));
      return [id, {
        present: Boolean(input),
        type: input?.getAttribute("type") ?? null,
        value: input?.value ?? null
      }];
    }));
    const resetButton = [...(section?.querySelectorAll("button") ?? [])].find((button) =>
      button.textContent?.trim() === "恢复默认外观"
    );
    return {
      present: Boolean(section),
      rangeSliderCount: section?.querySelectorAll('input[type="range"]').length ?? 0,
      colorInputCount: section?.querySelectorAll('input[type="color"]').length ?? 0,
      sliders,
      colors,
      resetButtonPresent: Boolean(resetButton),
      windowCornerRadius: getComputedStyle(document.documentElement)
        .getPropertyValue("--radius-window")
        .trim(),
      surfaceCornerRadius: getComputedStyle(document.documentElement)
        .getPropertyValue("--radius-panel")
        .trim(),
      sideBlur: getComputedStyle(document.documentElement)
        .getPropertyValue("--glass-side-blur")
        .trim(),
      centerBlur: getComputedStyle(document.documentElement)
        .getPropertyValue("--glass-center-blur")
        .trim(),
      dialogBlur: getComputedStyle(document.documentElement)
        .getPropertyValue("--glass-dialog-blur")
        .trim(),
      dialogBackdropDim: getComputedStyle(document.documentElement)
        .getPropertyValue("--dialog-backdrop-dim")
        .trim(),
      edgeHighlightBrightness: getComputedStyle(document.documentElement)
        .getPropertyValue("--glass-edge-highlight-brightness")
        .trim(),
      ditherOpacity: getComputedStyle(document.documentElement)
        .getPropertyValue("--glass-dither-opacity")
        .trim(),
      backdropGradientOpacity: getComputedStyle(document.documentElement)
        .getPropertyValue("--glass-backdrop-gradient-opacity")
        .trim()
    };
  })()`;
}

function glassMaterialEvidenceExpression() {
  return `(() => {
    const root = getComputedStyle(document.documentElement);
    const surface = (selector) => {
      const element = document.querySelector(selector);
      const style = element ? getComputedStyle(element) : null;
      const backdrop = element?.querySelector(":scope > .liquid-glass-backdrop") ?? null;
      const backdropStyle = backdrop ? getComputedStyle(backdrop) : null;
      return {
        present: Boolean(element),
        className: element?.className?.toString?.() ?? null,
        backgroundColor: style?.backgroundColor ?? null,
        backgroundImage: style?.backgroundImage ?? null,
        backdropLayerPresent: Boolean(backdrop),
        position: style?.position ?? null,
        backdropFilter: backdropStyle?.backdropFilter ?? null,
        webkitBackdropFilter: backdropStyle?.webkitBackdropFilter ?? null,
        glassBlurCurrent: style?.getPropertyValue("--glass-blur-current").trim() ?? null,
        pointerOpacity: style?.getPropertyValue("--glass-pointer-opacity").trim() ?? null,
        edgeHighlightBrightness: root.getPropertyValue("--glass-edge-highlight-brightness").trim()
      };
    };
    return {
      rootBackgrounds: {
        html: root.backgroundColor,
        body: getComputedStyle(document.body).backgroundColor,
        root: getComputedStyle(document.querySelector("#root")).backgroundColor,
        shell: getComputedStyle(document.querySelector(".app-shell")).backgroundColor
      },
      rootBackdropFilters: {
        html: root.backdropFilter,
        body: getComputedStyle(document.body).backdropFilter,
        root: getComputedStyle(document.querySelector("#root")).backdropFilter,
        shell: getComputedStyle(document.querySelector(".app-shell")).backdropFilter
      },
      variables: {
        sideBlur: root.getPropertyValue("--glass-side-blur").trim(),
        centerBlur: root.getPropertyValue("--glass-center-blur").trim(),
        dialogBlur: root.getPropertyValue("--glass-dialog-blur").trim(),
        dialogBackdropDim: root.getPropertyValue("--dialog-backdrop-dim").trim()
      },
      surfaces: {
        titlebarLeft: surface(".workbench-column-left"),
        titlebarCenter: surface(".workbench-column-center"),
        titlebarRight: surface(".workbench-column-right"),
        sidebar: surface(".workbench-column-left"),
        inspector: surface(".workbench-column-right"),
        popover: surface("#notifications-root, #filter-root, #sort-root")
      }
    };
  })()`;
}

function desktopNativeMaterialEvidence() {
  return inspectNativeBrowserWindowOptions(desktopMainSource);
}

function titlebarControlsEvidenceExpression() {
  return `(() => {
    const titlebar = document.querySelector(".shell-titlebar-center");
    const titlebarRect = titlebar?.getBoundingClientRect() ?? null;
    const controls = [...document.querySelectorAll(
      ".shell-titlebar-center .titlebar-icon-button, .shell-titlebar-center .filter-menu-trigger, .shell-titlebar-center .sort-menu-trigger"
    )].map((element) => {
      const style = getComputedStyle(element);
      const rect = element.getBoundingClientRect();
      const layer = (selector) => {
        const layerElement = element.querySelector(selector);
        const layerStyle = layerElement ? getComputedStyle(layerElement) : null;
        return {
          present: Boolean(layerElement),
          display: layerStyle?.display ?? null,
          visibility: layerStyle?.visibility ?? null,
          opacity: layerStyle?.opacity ?? null,
          filter: layerStyle?.filter ?? null,
          backdropFilter: layerStyle?.backdropFilter ?? null,
          webkitBackdropFilter: layerStyle?.webkitBackdropFilter ?? null
        };
      };
      return {
        className: element.className?.toString?.() ?? null,
        targetAttr: element.getAttribute("data-interactive-pointer-target"),
        interactiveOpacity: Number(style.getPropertyValue("--interactive-pointer-opacity") || 0),
        interactiveX: style.getPropertyValue("--interactive-pointer-x").trim(),
        interactiveY: style.getPropertyValue("--interactive-pointer-y").trim(),
        ariaLabel: element.getAttribute("aria-label"),
        title: element.getAttribute("title"),
        text: element.textContent?.trim() ?? "",
        iconCount: element.querySelectorAll("svg").length,
        glassPointerOpacity: Number(style.getPropertyValue("--glass-pointer-opacity") || 0),
        glassIllumination: Number(style.getPropertyValue("--glass-illumination") || 0),
        glassLensOpacity: Number(style.getPropertyValue("--glass-lens-opacity") || 0),
        backgroundColor: style.backgroundColor,
        backgroundImage: style.backgroundImage,
        border: style.border,
        borderTop: style.borderTop,
        boxShadow: style.boxShadow,
        layers: {
          backdrop: layer(".liquid-glass-backdrop"),
          lens: layer(".liquid-glass-lens"),
          edge: layer(".liquid-glass-edge")
        },
        rect: serializeRect(rect)
      };
    });
    const search = document.querySelector(".shell-titlebar-center .search-bar");
    const searchInput = document.querySelector(".shell-titlebar-center .search-bar .overlay-field-input");
    const searchRect = search?.getBoundingClientRect() ?? null;
    return {
      titlebarRect: titlebarRect ? serializeRect(titlebarRect) : null,
      controls,
      visibleTextLabels: controls.filter((control) => control.text.length > 0).map((control) => control.text),
      unframed: controls.every((control) =>
        control.boxShadow === "none" &&
        control.backgroundColor === "rgba(0, 0, 0, 0)" &&
        control.backgroundImage === "none" &&
        (/^0px\\b/.test(control.borderTop) || /\\bnone\\b/.test(control.borderTop))
      ),
      accessible: controls.every((control) => Boolean(control.ariaLabel && control.title)),
      iconOnly: controls.every((control) => control.iconCount > 0 && control.text.length === 0),
      search: {
        present: Boolean(search),
        inputPresent: Boolean(searchInput),
        rect: searchRect ? serializeRect(searchRect) : null,
        rightGap: titlebarRect && searchRect ? titlebarRect.right - searchRect.right : null,
        leftOfRightColumn: titlebarRect && searchRect ? searchRect.right <= titlebarRect.right + 1 : null
      }
    };
    function serializeRect(rect) {
      return { x: rect.x, y: rect.y, width: rect.width, height: rect.height, top: rect.top, right: rect.right, bottom: rect.bottom, left: rect.left };
    }
  })()`;
}

function titlebarAllButtonsEvidenceExpression() {
  return `(() => {
    const controls = [...document.querySelectorAll(
      '.shell-titlebar [data-liquid-glass="button"], .shell-titlebar .liquid-glass-button'
    )].map((element) => {
      const style = getComputedStyle(element);
      return {
        className: element.className?.toString?.() ?? null,
        targetAttr: element.getAttribute("data-interactive-pointer-target"),
        interactiveOpacity: Number(style.getPropertyValue("--interactive-pointer-opacity") || 0),
        interactiveX: style.getPropertyValue("--interactive-pointer-x").trim(),
        interactiveY: style.getPropertyValue("--interactive-pointer-y").trim()
      };
    });
    return { controls };
  })()`;
}

function notificationButtonStyleEvidenceExpression() {
  return `(() => {
    const notification = document.querySelector('[data-titlebar-control="notifications"]');
    const center = document.querySelector('.shell-titlebar-center .titlebar-icon-button');
    const inspect = (element) => {
      if (!element) return null;
      const style = getComputedStyle(element);
      const after = getComputedStyle(element, '::after');
      return {
        width: style.width,
        minWidth: style.minWidth,
        height: style.height,
        padding: style.padding,
        borderWidth: style.borderWidth,
        borderStyle: style.borderStyle,
        borderRadius: style.borderRadius,
        backgroundColor: style.backgroundColor,
        boxShadow: style.boxShadow,
        afterBorderWidth: after.borderWidth,
        afterBoxShadow: after.boxShadow,
        pointerTarget: element.getAttribute('data-interactive-pointer-target')
      };
    };
    const notificationStyle = inspect(notification);
    const centerStyle = inspect(center);
    const comparableProperties = [
      'width',
      'minWidth',
      'height',
      'padding',
      'borderWidth',
      'borderStyle',
      'borderRadius',
      'backgroundColor',
      'boxShadow',
      'afterBorderWidth',
      'afterBoxShadow',
      'pointerTarget'
    ];
    return {
      notification: notificationStyle,
      center: centerStyle,
      matchesCenter: Boolean(
        notificationStyle &&
        centerStyle &&
        comparableProperties.every((property) => notificationStyle[property] === centerStyle[property])
      ),
      statusDotCount: notification?.querySelectorAll('.notification-status-dot').length ?? 0
    };
  })()`;
}

function compactPopoverEvidenceExpression() {
  return `(() => {
    const notificationButtons = [...document.querySelectorAll('[data-titlebar-control="notifications"]')];
    const legacyButtons = [...document.querySelectorAll('[data-titlebar-control="tasks-palette"], [data-titlebar-control="recent-operations"]')];
    const notificationButton = notificationButtons[0] ?? null;
    const anchorRect = notificationButton?.getBoundingClientRect() ?? null;
    const roots = [...document.querySelectorAll("#notifications-root, #filter-root, #sort-root")].map((element) => {
      const style = getComputedStyle(element);
      const backdrop = element.querySelector(".liquid-glass-backdrop");
      const backdropStyle = backdrop ? getComputedStyle(backdrop) : null;
      const rect = element.getBoundingClientRect();
      const rootName = element.id.replace(/-root$/, "");
      const activeRows = [...element.querySelectorAll("[data-notification-active-operation]")];
      const thumbnailRows = [...element.querySelectorAll("[data-notification-thumbnail-library]")];
      const thumbnailList = element.querySelector("[data-notification-thumbnail-library-count]");
      const body = element.querySelector(".notification-popover-body");
      const footer = element.querySelector(".notification-popover-footer");
      const failure = element.querySelector(".notification-failure-cue");
      const empty = element.querySelector(".notification-popover-empty");
      const health = element.querySelector("[data-notification-health]");
      const sectionCount = element.querySelectorAll(".notification-section").length;
      const expectedSectionCount = Number(activeRows.length > 0) + Number(thumbnailRows.length > 0);
      const unexpectedBodyChildren = body
        ? [...body.children].filter((child) => !child.matches(
            ".notification-health-detail, .notification-failure-cue, .notification-section, .notification-popover-empty"
          )).length
        : 0;
      const bodyStyle = body ? getComputedStyle(body) : null;
      const footerRect = footer?.getBoundingClientRect() ?? null;
      const contentSections = [
        ["active", element.querySelector(".notification-active-list")],
        ["thumbnail", thumbnailList],
        ["failure", failure],
        ["empty", empty],
        ["center", element.querySelector(".notification-open-center")]
      ].filter(([, node]) => Boolean(node));
      return {
        name: rootName,
        className: element.className?.toString?.() ?? null,
        floating: style.position === "fixed" || style.position === "absolute",
        backgroundColor: style.backgroundColor,
        backgroundImage: style.backgroundImage,
        backdropLayerPresent: Boolean(backdrop),
        backdropFilter: backdropStyle?.backdropFilter ?? null,
        webkitBackdropFilter: backdropStyle?.webkitBackdropFilter ?? null,
        boxShadow: style.boxShadow,
        border: style.border,
        rect: { x: rect.x, y: rect.y, width: rect.width, height: rect.height, top: rect.top, right: rect.right, bottom: rect.bottom, left: rect.left },
        anchorRect: anchorRect
          ? { x: anchorRect.x, y: anchorRect.y, width: anchorRect.width, height: anchorRect.height, top: anchorRect.top, right: anchorRect.right, bottom: anchorRect.bottom, left: anchorRect.left }
          : null,
        anchorGeometry: anchorRect
          ? {
              gap: rect.top - anchorRect.bottom,
              rightDelta: rect.right - anchorRect.right,
              clampedLeft: rect.left <= 8.5,
              viewportContained: rect.left >= 0 && rect.top >= 0 && rect.right <= window.innerWidth && rect.bottom <= window.innerHeight
            }
          : null,
        notification: rootName === "notifications"
          ? {
              activeCount: activeRows.length,
              activeProgressCount: activeRows.filter((row) => row.querySelector(".notification-progress-track")).length,
              determinateCount: activeRows.filter((row) => row.querySelector(".notification-progress-percent")).length,
              indeterminateCount: activeRows.filter((row) => row.querySelector(".notification-progress-indeterminate")).length,
              phaseCount: element.querySelectorAll(".notification-operation-phase").length,
              progressLabelCount: element.querySelectorAll(".notification-progress-label").length,
              progressPercentCount: element.querySelectorAll(".notification-progress-percent").length,
              thumbnailCount: thumbnailRows.length,
              thumbnailLibraryCount: Number(thumbnailList?.getAttribute("data-notification-thumbnail-library-count") ?? 0),
              thumbnailSummaries: thumbnailRows.map((row) => ({
                libraryId: row.getAttribute("data-notification-thumbnail-library"),
                libraryName: row.getAttribute("data-notification-thumbnail-library-name"),
                progress: row.getAttribute("data-notification-thumbnail-progress"),
                label: row.querySelector(".notification-row-heading strong")?.textContent?.trim() ?? "",
                progressLabel: row.querySelector(".notification-thumbnail-progress-percent")?.textContent?.trim() ?? "",
                cachedTotal: row.querySelector(".notification-thumbnail-cached-total")?.textContent?.trim() ?? "",
                status: row.querySelector(".notification-thumbnail-status")?.textContent?.trim() ?? ""
              })),
              actionCount: element.querySelectorAll("[data-notification-action], .task-action, [data-action='cancel'], [data-action='retry']").length,
              fullCenterCount: element.querySelectorAll(".notification-open-center").length,
              contentOrder: contentSections.map(([name, node]) => ({ name, top: node.getBoundingClientRect().top })),
              onlyCurrentProgress: sectionCount === expectedSectionCount && unexpectedBodyChildren === 0,
              failure: Boolean(failure),
              failureCue: failure?.textContent?.trim() ?? "",
              health: health?.getAttribute("data-notification-health") ?? null,
              healthLabel: health?.textContent?.trim() ?? "",
              degraded: health?.getAttribute("data-notification-health") === "degraded",
              empty: Boolean(empty),
              bodyScrollable: bodyStyle?.overflowY === "auto" || bodyStyle?.overflowY === "scroll",
              footerVisible: Boolean(
                footerRect &&
                footerRect.top >= rect.top &&
                footerRect.bottom <= rect.bottom &&
                footerRect.bottom <= window.innerHeight
              ),
              boundedWidth: rect.width <= 361 && rect.width <= window.innerWidth
            }
          : null
      };
    });
    return {
      count: roots.length,
      names: roots.map((root) => root.name),
      roots,
      titlebar: {
        notificationButtonCount: notificationButtons.length,
        legacyButtonCount: legacyButtons.length,
        visibleText: notificationButtons.map((button) => button.textContent?.trim() ?? "")
      },
      viewport: { width: window.innerWidth, height: window.innerHeight }
    };
  })()`;
}

function pointerEvidenceExpression() {
  return `(() => {
    const surfaceState = (selector) => {
      const element = document.querySelector(selector);
      const style = element ? getComputedStyle(element) : null;
      return {
        dataset: element?.getAttribute("data-glass-pointer") ?? null,
        opacity: Number(style?.getPropertyValue("--glass-pointer-opacity") || 0),
        x: style?.getPropertyValue("--glass-pointer-x").trim() ?? null,
        y: style?.getPropertyValue("--glass-pointer-y").trim() ?? null
      };
    };
    const affordanceState = (selector) => {
      const element = document.querySelector(selector);
      const style = element ? getComputedStyle(element) : null;
      const before = element ? getComputedStyle(element, "::before") : null;
      const after = element ? getComputedStyle(element, "::after") : null;
      const rect = element?.getBoundingClientRect() ?? null;
      return {
        selector,
        targetAttr: element?.getAttribute("data-interactive-pointer-target") ?? null,
        interactiveOpacity: Number(style?.getPropertyValue("--interactive-pointer-opacity") || 0),
        interactiveX: style?.getPropertyValue("--interactive-pointer-x").trim() ?? null,
        interactiveY: style?.getPropertyValue("--interactive-pointer-y").trim() ?? null,
        beforeBackgroundImage: before?.backgroundImage ?? null,
        beforeBackgroundBlendMode: before?.backgroundBlendMode ?? null,
        beforeMaskImage: before?.maskImage ?? null,
        beforeWebkitMaskImage: before?.webkitMaskImage ?? null,
        afterBackgroundImage: after?.backgroundImage ?? null,
        afterMaskImage: after?.maskImage ?? null,
        afterWebkitMaskImage: after?.webkitMaskImage ?? null,
        rect: rect ? {
          left: rect.left,
          top: rect.top,
          width: rect.width,
          height: rect.height
        } : null
      };
    };
    const windowEdgeState = () => {
      const element = document.querySelector(".app-shell");
      const style = element ? getComputedStyle(element) : null;
      return {
        legacyFramePresent: Boolean(document.querySelector(".window-edge-frame")),
        borderRadius: style?.borderRadius ?? null,
        overflow: style?.overflow ?? null,
        borderWidth: style?.borderWidth ?? null
      };
    };
    return {
      windowEdge: windowEdgeState(),
      titlebarLeft: surfaceState(".workbench-column-left"),
      titlebarCenter: surfaceState(".workbench-column-center"),
      titlebarRight: surfaceState(".workbench-column-right"),
      inspector: surfaceState(".workbench-column-right"),
      sidebar: surfaceState(".workbench-column-left"),
      treeItem: affordanceState(".tree-item"),
      treeLabelButton: affordanceState(".tree-item .tree-label"),
      treeDisclosureButton: affordanceState(".tree-item .tree-disclosure"),
      tileThumb: affordanceState(".tile-thumb"),
      settingsSliderControl: affordanceState('[data-settings-slider-control="side-blur"]'),
      settingsSliderInput: affordanceState("#side-blur"),
      haloBrightness: getComputedStyle(document.documentElement)
        .getPropertyValue("--glass-halo-brightness")
        .trim(),
      edgeHighlightBrightness: getComputedStyle(document.documentElement)
        .getPropertyValue("--glass-edge-highlight-brightness")
        .trim()
    };
  })()`;
}

function isKnownWarning(text) {
  return /Electron Security Warning/i.test(text) && /Content-Security-Policy/i.test(text);
}

function isKnownConsoleError(text) {
  return /sandboxed_renderer\.bundle\.js script failed to run|preloadScripts.*startupData/i.test(text);
}

function isExpectedNetworkProblem(problem) {
  return problem.status === "loadingFailed" &&
    problem.errorText === "net::ERR_ABORTED" &&
    problem.canceled === true;
}

function isKnownStartupProblem(line) {
  return false;
}

function startupWarningsAndErrors() {
  return startupOutput
    .flatMap((entry) => entry.text.split(/\r?\n/).map((line) => ({ source: entry.source, line })))
    .filter(({ line }) => line.trim())
    .filter(({ line }) => {
      if (line.includes("[desktop-ui-regression] npm run dev exited")) return false;
      if (line.includes("Finished `dev` profile")) return false;
      if (line.includes("Running `target\\debug\\megle-core.exe`")) return false;
      if (line.includes("Running `target/debug/megle-core`")) return false;
      return /\b(warn|warning|error|failed|panic|unhandled)\b/i.test(line);
    })
    .map((problem) => ({ ...problem, known: isKnownStartupProblem(problem.line) }));
}

function near(actual, expected, tolerance) {
  return typeof actual === "number" && typeof expected === "number" && Math.abs(actual - expected) <= tolerance;
}

function transparent(value) {
  return value === "rgba(0, 0, 0, 0)" || value === "transparent";
}

function noBackdropFilter(value) {
  return !value || value === "none";
}

function effectiveBlurFilter(value) {
  if (!value || value === "none") return false;
  const match = value.match(/blur\(([-.\d]+)px\)/i);
  return Boolean(match && Number.parseFloat(match[1]) > 0);
}

function visibleBorder(border) {
  if (!border) return false;
  if (/^0px\b/.test(border) || /\b(?:none|hidden)\b/i.test(border)) return false;
  if (/\btransparent\b/i.test(border)) return false;
  const rgba = border.match(/rgba\([^)]*,\s*([.\d]+)\s*\)/i);
  if (rgba && Number.parseFloat(rgba[1]) <= 0.01) return false;
  return true;
}

function zeroRadius(value) {
  return typeof value === "string" && value.split(/\s+/).every((part) => {
    const number = Number.parseFloat(part);
    return Number.isFinite(number) && Math.abs(number) <= 0.01;
  });
}

function zeroCssLengths(value) {
  return typeof value === "string" && value.split(/\s+/).every((part) => {
    const number = Number.parseFloat(part);
    return Number.isFinite(number) && Math.abs(number) <= 0.01;
  });
}

function nonZeroRadius(value) {
  return typeof value === "string" && value.split(/\s+/).some((part) => {
    const number = Number.parseFloat(part);
    return Number.isFinite(number) && number > 0.01;
  });
}

function cssPercentNumber(value) {
  if (typeof value !== "string") return null;
  const number = Number.parseFloat(value);
  return Number.isFinite(number) ? number : null;
}

function nearCssPercent(value, expected, tolerance = 2) {
  const number = cssPercentNumber(value);
  return number !== null && Math.abs(number - expected) <= tolerance;
}

function pointerTracksPoint(state, point, tolerance = 2) {
  if (!state?.rect || !point) return false;
  const x = cssPercentNumber(state.interactiveX);
  const y = cssPercentNumber(state.interactiveY);
  return x !== null && y !== null &&
    near(x, point.x - state.rect.left, tolerance) &&
    near(y, point.y - state.rect.top, tolerance);
}

function rectWithin(inner, outer, tolerance = 2) {
  return Boolean(
    inner &&
      outer &&
      inner.left >= outer.left - tolerance &&
      inner.right <= outer.right + tolerance &&
      inner.top >= outer.top - tolerance &&
      inner.bottom <= outer.bottom + tolerance
  );
}

function hasInsetFocusRing(boxShadow) {
  if (!boxShadow || boxShadow === "none") return false;
  return /inset\b/i.test(boxShadow) && /\b1px\b/.test(boxShadow);
}

function hasVisibleOutline(outlineStyle, outlineWidth) {
  return outlineStyle !== "none" && outlineWidth !== "0px";
}

function serializeError(error) {
  if (error instanceof Error) {
    return {
      name: error.name,
      message: error.message,
      stack: error.stack ?? null
    };
  }
  return {
    name: "NonError",
    message: String(error),
    stack: null
  };
}

function assertCompactNotificationTask5Evidence(compactPopovers, fixture) {
  const failures = [];
  const stepByAction = new Map(
    (compactPopovers?.steps ?? []).map((step) => [step.action, step])
  );
  const notificationRoot = (evidence) =>
    evidence?.roots?.find((root) => root.name === "notifications") ?? null;
  const openRoot = notificationRoot(stepByAction.get("open-notifications")?.evidence);

  if (
    !openRoot?.notification?.onlyCurrentProgress ||
    openRoot.notification.actionCount !== 0 ||
    openRoot.notification.fullCenterCount !== 1 ||
    !openRoot.notification.bodyScrollable ||
    !openRoot.notification.footerVisible ||
    !openRoot.notification.boundedWidth
  ) {
    failures.push("production compact notification progress/layout contract failed");
  }

  for (const action of [
    "open-full-center-closes-notifications",
    "escape-closes-sort",
    "workspace-click-closes-notifications",
    "window-blur-closes-notifications"
  ]) {
    const step = stepByAction.get(action);
    if (!step || step.expected !== null || step.evidence?.count !== 0) {
      failures.push(`compact notification dismissal contract failed after ${action}`);
    }
  }

  const fixtureRoot = (evidence) => notificationRoot(evidence);
  const fullFixtureRoot = fixtureRoot(fixture?.full);
  const failureFixtureRoot = fixtureRoot(fixture?.failure);
  const emptyFixtureRoot = fixtureRoot(fixture?.empty);
  if (
    !fullFixtureRoot?.notification?.onlyCurrentProgress ||
    fullFixtureRoot.notification.actionCount !== 0 ||
    fullFixtureRoot.notification.fullCenterCount !== 1 ||
    !fullFixtureRoot.notification.bodyScrollable ||
    !fullFixtureRoot.notification.footerVisible ||
    !fullFixtureRoot.notification.boundedWidth
  ) {
    failures.push("full compact notification fixture layout contract failed");
  }
  if (
    !failureFixtureRoot?.notification?.failure ||
    failureFixtureRoot.notification.failureCue !== "部分任务失败，请打开完整通知中心查看" ||
    failureFixtureRoot.notification.health !== "failure" ||
    failureFixtureRoot.notification.empty ||
    failureFixtureRoot.notification.activeCount !== 0 ||
    failureFixtureRoot.notification.thumbnailCount !== 0 ||
    !failureFixtureRoot.notification.onlyCurrentProgress ||
    failureFixtureRoot.notification.fullCenterCount !== 1
  ) {
    failures.push("failure-only compact notification fixture contract failed");
  }
  if (
    !emptyFixtureRoot?.notification?.empty ||
    emptyFixtureRoot.notification.failure ||
    emptyFixtureRoot.notification.health !== "healthy" ||
    emptyFixtureRoot.notification.activeCount !== 0 ||
    emptyFixtureRoot.notification.thumbnailCount !== 0 ||
    emptyFixtureRoot.notification.fullCenterCount !== 1
  ) {
    failures.push("empty compact notification fixture contract failed");
  }

  if (failures.length > 0) {
    throw new Error(`Task5 compact notification assertions failed: ${failures.join("; ")}`);
  }
  console.log("PASS: Task5 compact notification assertions");
}

startDevApp();
let client;
let summary = null;
let fatalError = null;
let hardFailures = [];

try {
try {
  const target = await waitForTarget();
  client = new CdpClient(target.webSocketDebuggerUrl);
  await client.ready;

  client.on("Runtime.consoleAPICalled", (event) => {
    if (event.type !== "warning" && event.type !== "error") return;
    const text = event.args?.map((arg) => arg.value ?? arg.description ?? "").join(" ") ?? "";
    if (event.type === "warning") consoleWarnings.push({ type: event.type, text, known: isKnownWarning(text) });
    else consoleErrors.push({ type: event.type, text });
  });
  client.on("Log.entryAdded", (event) => {
    const entry = event.entry ?? {};
    if (entry.level === "warning") consoleWarnings.push({ type: "log", text: entry.text ?? "", known: isKnownWarning(entry.text ?? "") });
    if (entry.level === "error") consoleErrors.push({ type: "log", text: entry.text ?? "" });
  });
  client.on("Network.responseReceived", (event) => {
    const response = event.response ?? {};
    responses.push({ url: response.url, status: response.status, type: event.type });
    if (response.status >= 400) {
      networkProblems.push({ url: response.url, status: response.status, type: event.type });
    }
  });
  client.on("Network.requestWillBeSent", (event) => {
    requestUrls.set(event.requestId, event.request?.url ?? null);
  });
  client.on("Network.loadingFailed", (event) => {
    if (event.type !== "WebSocket") {
      networkProblems.push({
        requestId: event.requestId,
        url: requestUrls.get(event.requestId) ?? null,
        status: "loadingFailed",
        type: event.type,
        errorText: event.errorText,
        canceled: event.canceled === true,
        blockedReason: event.blockedReason ?? null
      });
    }
  });

  await Promise.all([
    client.send("Page.enable"),
    client.send("Runtime.enable"),
    client.send("Network.enable"),
    client.send("Log.enable")
  ]);
  await client.send("Emulation.setDefaultBackgroundColorOverride", {
    color: { r: 0, g: 0, b: 0, a: 0 }
  });

  if (notificationWindowOnly) {
    await waitFor(client, `Boolean(document.querySelector(".app-shell"))`, 30000, "app shell");
    const notificationWindowFixture = await notificationWindowFixtureEvidence(client);
    const notificationFailures = [];
    if (
      notificationWindowFixture.status?.tabOrientation !== "vertical" ||
      notificationWindowFixture.narrow?.tabOrientation !== "horizontal"
    ) {
      notificationFailures.push("notification tab orientation did not follow the responsive layout");
    }
    if (
      notificationWindowFixture.firstFailureDrilldown?.visibleFailureRows < 1 ||
      notificationWindowFixture.firstFailureDrilldown?.visibleActionableRows < 1 ||
      !notificationWindowFixture.firstFailureDrilldown?.actionMessage?.includes("媒体")
    ) {
      notificationFailures.push("first failure drilldown was not visible and actionable");
    }
    summary = {
      run: {
        id: visualRunId,
        mode: "task6-notification-window",
        startedAtUtc: runStartedAt.toISOString(),
        completedAtUtc: new Date().toISOString()
      },
      screenshots: notificationWindowFixture.screenshots,
      evidence: { notificationWindowFixture },
      hardFailures: notificationFailures,
      fatalError: null
    };
    await writeFile(summaryPath, JSON.stringify(summary, null, 2));
    if (notificationFailures.length) {
      throw new Error(`Task6 notification window harness failed: ${notificationFailures.join(", ")}`);
    }
  } else {

  await waitFor(client, `Boolean(document.querySelector(".app-shell"))`, 30000, "app shell");
  await waitFor(
    client,
    `[...document.querySelectorAll(".tile-label")].some((item) => item.textContent?.includes("landscape-sample")) &&
      [...document.querySelectorAll(".tile-label")].some((item) => item.textContent?.includes("portrait-sample"))`,
    90000,
    "sample media tiles"
  );
  await waitFor(client, `document.querySelectorAll(".tile-thumb-image").length >= 2`, 90000, "ready thumbnails");

  const integratedTitlebarMain = await screenshot(client, "ui-integrated-titlebar-main.png");
  const layout = await evaluate(client, layoutEvidenceExpression());
  const titlebarControls = await evaluate(client, titlebarControlsEvidenceExpression());
  const titlebarToolSelector =
    ".shell-titlebar-center .filter-menu-trigger:not(:disabled)";
  await forcePseudoState(client, titlebarToolSelector, ["hover"]);
  await moveToSelector(
    client,
    titlebarToolSelector,
    0.5,
    0.5
  );
  const titlebarToolGlassActive = await evaluate(client, titlebarControlsEvidenceExpression());
  await forcePseudoState(client, titlebarToolSelector, []);
  await moveToSelector(client, ".shell-titlebar-center", 0.055, 0.5);
  const titlebarToolGlassNearby = await evaluate(client, titlebarControlsEvidenceExpression());
  await moveToSelector(client, ".shell-titlebar-right", 0.78, 0.5);
  const titlebarAllButtonsNearby = await evaluate(client, titlebarAllButtonsEvidenceExpression());
  const nativeMaterial = desktopNativeMaterialEvidence();
  const windowBackdrop = await captureWindowBackdropEvidence(
    client,
    `ui-window-backdrop-evidence-${visualRunId}.png`,
    layout
  );
  const titlebarDrag = await titlebarDragEvidence(client);
  const notificationMaximize = await notificationMaximizeAnchorEvidence(client);
  const searchNoDrag = await searchNoDragEvidence(client);
  const settingsInterfaceStyle = await settingsInterfaceStyleEvidence(client);
  const pluginsSecondaryWindow = await pluginsSecondaryWindowEvidence(client);
  const notificationsSecondaryWindow = await notificationsSecondaryWindowEvidence(client);
  const notificationButtonStyle = await evaluate(
    client,
    notificationButtonStyleEvidenceExpression()
  );
  const compactPopovers = await compactPopoverDismissalEvidence(client);
  const notificationFixture = await notificationPopoverFixtureEvidence(client);
  assertCompactNotificationTask5Evidence(compactPopovers, notificationFixture);

  await selectMediaByName(client, "portrait-sample");
  await waitFor(
    client,
    `(() => {
      const image = document.querySelector(".inspector-panel .preview-stage .preview-image");
      return Boolean(image && image.complete && image.naturalWidth > 0 && image.naturalHeight > 0);
    })()`,
    30000,
    "portrait inspector preview image"
  );
  const selectedPortraitRightPreview = await screenshot(client, "ui-selected-portrait-right-preview.png");
  const inspectorPortrait = await evaluate(client, inspectorPreviewEvidenceExpression());
  const inspectorPreviewApiResponsesBeforeCentralOpen = responses.filter(isPreviewApiResponse);
  const inspectorThumbnailBlobResponsesBeforeCentralOpen = responses.filter(isThumbnailBlobResponse);

  await doubleClickMediaByName(client, "landscape-sample");
  try {
    await waitFor(
      client,
      `(() => {
        const ready = document.querySelector(".central-preview-stage .preview-placeholder.ready");
        const image = document.querySelector(".central-preview-stage .preview-image");
        const rect = image?.getBoundingClientRect();
        const isCanvas = image instanceof HTMLCanvasElement;
        return Boolean(
          ready &&
            image &&
            rect &&
            rect.width > 0 &&
            rect.height > 0 &&
            (isCanvas
              ? image.width > 0 && image.height > 0
              : image.complete && image.naturalWidth > 0 && image.naturalHeight > 0)
        );
      })()`,
      30000,
      "landscape central preview image"
    );
  } catch (error) {
    const diagnosticPath = path.join(logDir, "desktop-ui-regression-landscape-timeout.json");
    const diagnostic = {
      capturedAtUtc: new Date().toISOString(),
      page: await evaluate(
        client,
        `(() => {
          const readImage = (image) => image ? {
            alt: image.getAttribute("alt"),
            className: image.className?.toString?.() ?? null,
            complete: image.complete,
            naturalWidth: image.naturalWidth,
            naturalHeight: image.naturalHeight,
            canvasWidth: image instanceof HTMLCanvasElement ? image.width : null,
            canvasHeight: image instanceof HTMLCanvasElement ? image.height : null,
            rect: (() => {
              const rect = image.getBoundingClientRect();
              return { width: rect.width, height: rect.height };
            })(),
            src: image.currentSrc || image.getAttribute("src") || null
          } : null;
          return {
            url: location.href,
            selectedTile: document.querySelector(".media-tile.selected .tile-label")?.textContent?.trim() ?? null,
            selectedMediaIds: [...document.querySelectorAll(".media-tile.selected")].map((tile) => tile.getAttribute("data-media-id")),
            centralStage: Boolean(document.querySelector(".central-preview-stage")),
            previewPlaceholders: [...document.querySelectorAll(".central-preview-stage .preview-placeholder")].map((node) => ({
              className: node.className,
              state: node.getAttribute("data-preview-state"),
              source: node.getAttribute("data-preview-source"),
              text: node.textContent?.trim() ?? null
            })),
            previewImages: [...document.querySelectorAll(".central-preview-stage .preview-image")].map(readImage),
            title: document.querySelector(".titlebar-preview-title")?.textContent?.trim() ?? null,
            bodyText: document.querySelector(".central-preview-stage")?.textContent?.trim()?.slice(0, 500) ?? null
          };
        })()`
      ),
      previewResponses: responses.filter(isPreviewApiResponse),
      previewFailures: networkProblems.filter((item) => item.url?.includes("/api/media/") && item.url?.includes("/preview")),
      consoleErrors,
      error: String(error)
    };
    await writeFile(diagnosticPath, JSON.stringify(diagnostic, null, 2));
    throw new Error(`${String(error)}; diagnostic=${diagnosticPath}`);
  }
  await delay(500);
  const centralLandscapeFit = await screenshot(client, "ui-central-landscape-fit-long-edge.png");
  const landscapeFit = await evaluate(client, centralPreviewEvidenceExpression());
  const layoutDuringLandscapePreview = await evaluate(client, layoutEvidenceExpression());
  const landscapeToActualAnchor = await doubleClickStage(client);
  const centralLandscapeActual = await screenshot(client, "ui-central-landscape-actual-100.png");
  const landscapeActual = await evaluate(client, centralPreviewEvidenceExpression());
  const landscapeToFitAnchor = await doubleClickStage(client);
  const landscapeFitAgain = await evaluate(client, centralPreviewEvidenceExpression());

  await closeCentralPreview(client);
  await waitFor(client, `!document.querySelector(".central-preview-stage")`, 10000, "central preview close");

  await doubleClickMediaByName(client, "portrait-sample");
  await waitFor(
    client,
    `(() => {
      const ready = document.querySelector(".central-preview-stage .preview-placeholder.ready");
      const image = document.querySelector(".central-preview-stage .preview-image");
      const rect = image?.getBoundingClientRect();
      const isCanvas = image instanceof HTMLCanvasElement;
      return Boolean(
        ready &&
          image &&
          rect &&
          rect.width > 0 &&
          rect.height > 0 &&
          (isCanvas
            ? image.width > 0 && image.height > 0
            : image.complete && image.naturalWidth > 0 && image.naturalHeight > 0)
      );
    })()`,
    30000,
    "portrait central preview image"
  );
  await delay(500);
  const centralPortraitFit = await screenshot(client, "ui-central-portrait-fit-long-edge.png");
  const portraitFit = await evaluate(client, centralPreviewEvidenceExpression());

  await closeCentralPreview(client);
  await waitFor(client, `!document.querySelector(".central-preview-stage")`, 10000, "central preview close after portrait");

  const pointFarIdle = await moveToSelector(client, ".grid-surface", 0.5, 0.5);
  const pointerFarIdle = await evaluate(client, pointerEvidenceExpression());
  const pointerFarIdleScreenshot = await screenshot(client, "ui-pointer-far-from-edge-idle.png");
  const pointWindowOuterEdge = await moveToSelector(client, ".app-shell", 0.5, 0.01);
  const pointerWindowOuterEdge = await evaluate(client, pointerEvidenceExpression());
  const pointerWindowOuterEdgeScreenshot = await screenshot(client, "ui-window-outer-edge-highlight.png");
  const pointNearEdge = await moveToSelector(client, ".inspector-panel", 0.5, 0.985);
  const pointerNearEdge = await evaluate(client, pointerEvidenceExpression());
  const pointTreeItem = await moveToSelector(client, ".tree-item", 0.35, 0.6);
  const pointerTreeItem = await evaluate(client, pointerEvidenceExpression());
  const pointTileThumb = await moveToSelector(client, ".tile-thumb", 0.35, 0.72);
  const pointerTileThumb = await evaluate(client, pointerEvidenceExpression());
  const localEdgeHighlight = await screenshot(client, "ui-local-edge-highlight.png");
  const notificationWindowFixture = await notificationWindowFixtureEvidence(client);

  summary = {
    run: {
      id: visualRunId,
      startedAtUtc: runStartedAt.toISOString(),
      completedAtUtc: new Date().toISOString()
    },
    screenshots: {
      integratedTitlebarMain,
      settingsInterfaceStyle: settingsInterfaceStyle.screenshotPath,
      pluginsSecondaryWindow: pluginsSecondaryWindow.screenshotPath,
      notificationsSecondaryWindow: notificationsSecondaryWindow.screenshotPath,
      notificationsSecondaryWindowLogs: notificationsSecondaryWindow.logsScreenshotPath,
      selectedPortraitRightPreview,
      centralLandscapeFit,
      centralLandscapeActual,
      centralPortraitFit,
      pointerFarIdle: pointerFarIdleScreenshot,
      pointerWindowOuterEdge: pointerWindowOuterEdgeScreenshot,
      localEdgeHighlight,
      notificationFixture: notificationFixture.screenshots,
      notificationWindowFixture: notificationWindowFixture.screenshots,
      windowBackdrop: windowBackdrop.path ?? null
    },
    verification: {
      osBackdrop: osBackdropVerificationSummary(windowBackdrop)
    },
    evidence: {
      layout,
      nativeMaterial,
      windowBackdrop,
      titlebarControls,
      titlebarToolGlassActive,
      titlebarToolGlassNearby,
      titlebarAllButtonsNearby,
      titlebarDrag,
      notificationMaximize,
      searchNoDrag,
      settingsInterfaceStyle: settingsInterfaceStyle.evidence,
      pluginsSecondaryWindow,
      notificationsSecondaryWindow,
      notificationButtonStyle,
      compactPopovers,
      notificationFixture,
      notificationWindowFixture,
      inspectorPortrait,
      inspectorPreviewApiResponsesBeforeCentralOpen,
      inspectorThumbnailBlobResponsesBeforeCentralOpen,
      landscapeFit,
      layoutDuringLandscapePreview,
      landscapeActual,
      landscapeFitAgain,
      landscapeToActualAnchor,
      landscapeToFitAnchor,
      portraitFit,
      pointerPoints: {
        farIdle: pointFarIdle,
        windowOuterEdge: pointWindowOuterEdge,
        nearEdge: pointNearEdge,
        treeItem: pointTreeItem,
        tileThumb: pointTileThumb
      },
      pointerFarIdle,
      pointerWindowOuterEdge,
      pointerNearEdge,
      pointerTreeItem,
      pointerTileThumb,
      previewApiResponses: responses.filter(isPreviewApiResponse),
      thumbnailBlobResponses: responses.filter(isThumbnailBlobResponse)
    },
    consoleWarnings,
    consoleErrors,
    networkProblems,
    startupProblems: startupWarningsAndErrors(),
    fatalError: null,
    hardFailures,
    stdoutPath,
    stderrPath
  };
  await writeFile(summaryPath, JSON.stringify(summary, null, 2));

  hardFailures = [];
  const unknownConsoleWarnings = consoleWarnings.filter((warning) => !warning.known);
  const unexpectedConsoleErrors = consoleErrors.filter((entry) => !isKnownConsoleError(entry.text ?? ""));
  const unexpectedNetworkProblems = networkProblems.filter((problem) => !isExpectedNetworkProblem(problem));
  const unknownStartupProblems = summary.startupProblems.filter((problem) => !problem.known);
  if (unexpectedConsoleErrors.length) hardFailures.push("console errors");
  if (unexpectedNetworkProblems.length) hardFailures.push("network failures");
  if (unknownConsoleWarnings.length) hardFailures.push("unknown console warnings");
  if (unknownStartupProblems.length) hardFailures.push("unknown startup warnings/errors");
  if (!transparent(layout.htmlBackground) || !transparent(layout.bodyBackground) || !transparent(layout.rootBackground)) {
    hardFailures.push("html/body/root backgrounds are not transparent");
  }
  const stagedSecondaryReveal = (evidence) => {
    const nativeWarmupStartedAt = Number(evidence?.nativeWarmupStartedAt);
    const nativeWindowRevealedAt = Number(evidence?.nativeWindowRevealedAt);
    const contentVisibleAt = Number(evidence?.contentVisibleAt);
    return (
      Number.isFinite(nativeWarmupStartedAt) &&
      Number.isFinite(nativeWindowRevealedAt) &&
      Number.isFinite(contentVisibleAt) &&
      nativeWarmupStartedAt <= nativeWindowRevealedAt &&
      nativeWindowRevealedAt <= contentVisibleAt &&
      nativeWindowRevealedAt - nativeWarmupStartedAt >= 100 &&
      evidence?.contentVisible === "true" &&
      evidence?.revealFallback === "false"
    );
  };
  if (
    !noBackdropFilter(layout.htmlBackdropFilter) ||
    !noBackdropFilter(layout.bodyBackdropFilter) ||
    !noBackdropFilter(layout.rootBackdropFilter) ||
    !noBackdropFilter(layout.shellBackdropFilter)
  ) {
    hardFailures.push("html/body/root/app-shell must not apply backdrop-filter");
  }
  if (!transparent(layout.shellBackground) || layout.shellBackgroundImage !== "none") {
    hardFailures.push("app shell still paints a root rectangle");
  }
  if (
    nativeMaterial.backgroundMaterial !== "acrylic" ||
    !nativeMaterial.frameFalse ||
    !nativeMaterial.nonLayeredHost ||
    !nativeMaterial.transparentBackgroundColor ||
    !nativeMaterial.roundedCorners ||
    nativeMaterial.disablesNativeMaterial ||
    nativeMaterial.unsafeTopLevelSpreads.length > 0
  ) {
    hardFailures.push("desktop BrowserWindow must use native acrylic with non-layered DWM-rounded frameless window settings");
  }
  if (
    osBackdropEvidenceEnabled &&
    (
      windowBackdrop.skipped ||
      !windowBackdrop.path ||
      typeof windowBackdrop.maxDelta !== "number" ||
      typeof windowBackdrop.bottomMaxDelta !== "number" ||
      typeof windowBackdrop.uiSurfaceDelta !== "number"
    )
  ) {
    hardFailures.push("MEGLE_VISUAL_OS_BACKDROP=1 did not produce OS-composited window backdrop evidence");
  }
  if (!windowBackdrop.skipped && windowBackdrop.uiSurfaceDelta < 18) {
    hardFailures.push("desktop screenshot did not capture visible Megle UI surfaces for backdrop comparison");
  } else if (!windowBackdrop.skipped && !backdropPixelEvidencePassed(windowBackdrop)) {
    hardFailures.push("OS backdrop pixel evidence does not show transparent passthrough or native acrylic-tinted desktop variation");
  }
  if (!layout.titlebarLeft || !layout.titlebarCenter || !layout.titlebarRight) {
    hardFailures.push("integrated titlebar columns are missing");
  }
  if (
    !near(layout.titlebarLeft?.top, 0, 1) ||
    !near(layout.titlebarCenter?.top, 0, 1) ||
    !near(layout.titlebarRight?.top, 0, 1)
  ) {
    hardFailures.push("integrated titlebar columns do not reach the top of the window");
  }
  if (
    !near(layout.sidebar?.top, layout.titlebarLeft?.bottom, 2) ||
    !near(layout.sidebar?.left, layout.titlebarLeft?.left, 2) ||
    !near(layout.sidebar?.right, layout.titlebarLeft?.right, 2)
  ) {
    hardFailures.push("sidebar does not visually connect below the left titlebar");
  }
  if (
    !near(layout.inspector?.top, layout.titlebarRight?.bottom, 2) ||
    !near(layout.inspector?.left, layout.titlebarRight?.left, 2) ||
    !near(layout.inspector?.right, layout.titlebarRight?.right, 2)
  ) {
    hardFailures.push("right titlebar is not aligned with the inspector column");
  }
  if (
    !near(layout.gridSurface?.top, layout.titlebarCenter?.bottom, 2) ||
    !near(layout.gridSurface?.left, layout.titlebarCenter?.left, 2) ||
    !near(layout.gridSurface?.right, layout.titlebarRight?.left, 2)
  ) {
    hardFailures.push("center titlebar is not aligned with the workspace grid area");
  }
  if (
    layout.gridSurfaceStyle?.dataLiquidGlass !== null ||
    layout.gridSurfaceStyle?.backdropLayerPresent
  ) {
    hardFailures.push("center content column must stay a plain content stage instead of a structural LiquidGlass surface");
  }
  if (
    visibleBorder(layout.titlebarLeftStyle?.borderBottom) ||
    visibleBorder(layout.sidebarStyle?.borderTop)
  ) {
    hardFailures.push("left titlebar/sidebar internal join still draws a visible border");
  }
  if (
    visibleBorder(layout.titlebarCenterStyle?.borderBottom) ||
    visibleBorder(layout.gridSurfaceStyle?.borderTop)
  ) {
    hardFailures.push("center titlebar/workspace join still draws a seam or double border");
  }
  if (
    visibleBorder(layout.gridSurfaceAfterStyle?.borderTop) ||
    visibleBorder(layout.gridSurfaceAfterStyle?.borderLeft) ||
    visibleBorder(layout.gridSurfaceAfterStyle?.borderRight)
  ) {
    hardFailures.push("center content LiquidGlass surface draws internal top or duplicate side borders");
  }
  if (
    visibleBorder(layout.titlebarRightStyle?.borderBottom) ||
    visibleBorder(layout.inspectorStyle?.borderTop)
  ) {
    hardFailures.push("right titlebar/inspector internal join still draws a visible border");
  }
  if (layout.joinGaps?.left > 1 || layout.joinGaps?.center > 1 || layout.joinGaps?.right > 1) {
    hardFailures.push("titlebar/content material joins have a visible gap");
  }
  if (
    !near(layoutDuringLandscapePreview.gridSurface?.top, layoutDuringLandscapePreview.titlebarCenter?.bottom, 1) ||
    visibleBorder(layoutDuringLandscapePreview.titlebarCenterStyle?.borderBottom) ||
    visibleBorder(layoutDuringLandscapePreview.gridSurfaceStyle?.borderTop) ||
    layoutDuringLandscapePreview.gridSurfaceStyle?.dataLiquidGlass !== null ||
    layoutDuringLandscapePreview.gridSurfaceStyle?.backdropLayerPresent
  ) {
    hardFailures.push("center titlebar/content join is not seamless while central preview is open");
  }
  if (!summary.evidence.titlebarControls.iconOnly || summary.evidence.titlebarControls.visibleTextLabels.length > 0) {
    hardFailures.push("middle titlebar tool buttons are not icon-only");
  }
  if (!summary.evidence.titlebarControls.unframed) {
    hardFailures.push("middle titlebar tool buttons still have persistent frames");
  }
  if (!summary.evidence.titlebarControls.accessible) {
    hardFailures.push("middle titlebar tool buttons are missing accessible labels or titles");
  }
  const activeTitlebarToolGlass = summary.evidence.titlebarToolGlassActive.controls?.find(
    (control) => control.targetAttr === "true" && control.interactiveOpacity > 0
  );
  if (!activeTitlebarToolGlass) {
    hardFailures.push("middle titlebar tool hover did not activate the unified pointer highlight");
  }
  const nearbyTitlebarToolGlass = summary.evidence.titlebarToolGlassNearby.controls?.find(
    (control) => control.targetAttr === "true" && control.interactiveOpacity > 0
  );
  if (!nearbyTitlebarToolGlass) {
    hardFailures.push("titlebar buttons did not refresh unified pointer highlight from nearby pointer movement");
  }
  const nearbyAnyTitlebarButton = summary.evidence.titlebarAllButtonsNearby.controls?.find(
    (control) => control.targetAttr === "true" && control.interactiveOpacity > 0
  );
  if (!nearbyAnyTitlebarButton) {
    hardFailures.push("titlebar right-side buttons did not refresh unified pointer highlight");
  }
  if (
    !summary.evidence.titlebarControls.search.present ||
    !summary.evidence.titlebarControls.search.inputPresent ||
    !(summary.evidence.titlebarControls.search.rightGap >= 0 && summary.evidence.titlebarControls.search.rightGap <= 28)
  ) {
    hardFailures.push("middle titlebar search is missing or not right-aligned");
  }
  if (!summary.evidence.titlebarDrag.bridgeAvailable) {
    hardFailures.push("desktop windowControls bridge is unavailable in Electron visual harness");
  }
  if (summary.evidence.titlebarDrag.point?.titlebarAppRegion !== "drag") {
    hardFailures.push("center titlebar surface must use the native Electron drag region");
  }
  if (!summary.evidence.searchNoDrag.focused) {
    hardFailures.push("search input in center titlebar did not receive focus");
  }
  if (summary.evidence.searchNoDrag.appRegion !== "no-drag") {
    hardFailures.push("search input in center titlebar is not marked no-drag");
  }
  const pluginsWindowEvidence = summary.evidence.pluginsSecondaryWindow;
  if (
    pluginsWindowEvidence.mainBefore?.oldPrimaryNavPresent ||
    pluginsWindowEvidence.mainBefore?.pluginsInMain ||
    !pluginsWindowEvidence.mainBefore?.libraryVisible ||
    pluginsWindowEvidence.mainAfter?.pluginsInMain ||
    !pluginsWindowEvidence.mainAfter?.libraryVisible
  ) {
    hardFailures.push("Plugins secondary window replaced main Library content or old primary nav is still present");
  }
  if (
    JSON.stringify(pluginsWindowEvidence.mainBefore?.controlOrder) !==
    JSON.stringify(["library-switcher", "open-plugins", "open-settings", "locate-folder", "toggle-sidebars"]) ||
    JSON.stringify(pluginsWindowEvidence.mainBefore?.utilityOrder) !==
      JSON.stringify(["open-plugins", "open-settings", "locate-folder", "toggle-sidebars"]) ||
    pluginsWindowEvidence.mainBefore?.utilityRightGap > 13 ||
    pluginsWindowEvidence.mainBefore?.libraryWidth >= 168 ||
    pluginsWindowEvidence.mainBefore?.treeTopGap > 6
  ) {
    hardFailures.push("titlebar utility grouping, intrinsic library width, or sidebar tree top spacing is incorrect");
  }
  for (const [name, control] of Object.entries(pluginsWindowEvidence.mainBefore?.controls ?? {})) {
    if (
      control?.borderWidth !== "0px" ||
      control?.afterBorderWidth !== "0px" ||
      control?.afterBoxShadow !== "none" ||
      control?.pointerTarget !== "true" ||
      !/radial-gradient/i.test(control?.beforeBackgroundImage ?? "") ||
      !/radial-gradient/i.test(control?.afterBackgroundImage ?? "")
    ) {
      hardFailures.push(`${name} titlebar control lost pointer material or retained a static outline`);
    }
  }
  if (
    pluginsWindowEvidence.firstTargetId !== pluginsWindowEvidence.repeatedTargetId ||
    pluginsWindowEvidence.coexistence?.plugins !== 1 ||
    pluginsWindowEvidence.coexistence?.settings !== 1 ||
    pluginsWindowEvidence.pluginWindow?.role !== "plugins" ||
    !pluginsWindowEvidence.pluginWindow?.closePresent ||
    !pluginsWindowEvidence.pluginWindow?.pluginSurfacePresent ||
    pluginsWindowEvidence.pluginWindow?.rendererReady !== "true" ||
    pluginsWindowEvidence.pluginWindow?.windowVisible !== "true" ||
    !stagedSecondaryReveal(pluginsWindowEvidence.pluginWindow) ||
    pluginsWindowEvidence.settingsWindow?.rendererReady !== "true" ||
    pluginsWindowEvidence.settingsWindow?.windowVisible !== "true" ||
    !stagedSecondaryReveal(pluginsWindowEvidence.settingsWindow) ||
    JSON.stringify(pluginsWindowEvidence.pluginWindow?.material) !==
      JSON.stringify(pluginsWindowEvidence.settingsWindow?.material)
  ) {
    hardFailures.push("Plugins window single-instance/coexistence/renderer-role contract failed");
  }
  const notificationsWindowEvidence = summary.evidence.notificationsSecondaryWindow;
  const notificationPositionPersisted =
    notificationsWindowEvidence.movedBounds &&
    notificationsWindowEvidence.restoredBounds &&
    Math.abs(
      notificationsWindowEvidence.movedBounds.x - notificationsWindowEvidence.restoredBounds.x
    ) <= 2 &&
    Math.abs(
      notificationsWindowEvidence.movedBounds.y - notificationsWindowEvidence.restoredBounds.y
    ) <= 2;
  if (
    notificationsWindowEvidence.firstTargetId !== notificationsWindowEvidence.repeatedTargetId ||
    notificationsWindowEvidence.window?.role !== "notifications" ||
    notificationsWindowEvidence.window?.title !== "通知中心" ||
    !notificationsWindowEvidence.window?.closePresent ||
    notificationsWindowEvidence.window?.rendererReady !== "true" ||
    notificationsWindowEvidence.window?.windowVisible !== "true" ||
    !stagedSecondaryReveal(notificationsWindowEvidence.window) ||
    JSON.stringify(notificationsWindowEvidence.window?.material) !==
      JSON.stringify(pluginsWindowEvidence.pluginWindow?.material) ||
    !notificationPositionPersisted
  ) {
    hardFailures.push("Notification window single-instance/position/material/reveal contract failed");
  }
  const notificationPages = notificationsWindowEvidence.pages;
  if (
    notificationPages?.status?.tabCount !== 3 ||
    notificationPages.status.selectedTab !== "notification-tab-status" ||
    !notificationPages.status.bodyBackground?.includes("gradient") ||
    !/^0px none/.test(notificationPages.status.navBorderRight ?? "") ||
    notificationPages.status.contentOverflow !== "hidden" ||
    !/^1px solid/.test(notificationPages.status.titlebarBorderBottom ?? "") ||
    notificationPages.history?.filterCount < 5 ||
    notificationPages.history?.unreadControls !== 0 ||
    notificationPages.history?.virtualRows >= 100 ||
    !notificationPages.logs?.sourceFilter ||
    !notificationPages.logs?.levelFilter ||
    !notificationPages.logs?.pause ||
    !notificationPages.logs?.follow ||
    !notificationPages.logs?.copy ||
    !notificationPages.logs?.clear ||
    !notificationPages.logs?.export ||
    notificationPages.logs?.virtualLines >= 100
  ) {
    hardFailures.push("Notification window status/history/log page contract failed");
  }
  if (
    !summary.evidence.settingsInterfaceStyle.present ||
    summary.evidence.settingsInterfaceStyle.rangeSliderCount < 29 ||
    summary.evidence.settingsInterfaceStyle.colorInputCount < 2 ||
    !summary.evidence.settingsInterfaceStyle.sliders?.["window-corner-radius"]?.present ||
    !summary.evidence.settingsInterfaceStyle.sliders?.["surface-corner-radius"]?.present ||
    !summary.evidence.settingsInterfaceStyle.sliders?.["control-corner-radius"]?.present ||
    !summary.evidence.settingsInterfaceStyle.sliders?.["content-corner-radius"]?.present ||
    !summary.evidence.settingsInterfaceStyle.sliders?.["side-blur"]?.present ||
    !summary.evidence.settingsInterfaceStyle.sliders?.["side-opacity"]?.present ||
    !summary.evidence.settingsInterfaceStyle.sliders?.["side-overlay-strength"]?.present ||
    !summary.evidence.settingsInterfaceStyle.colors?.["side-overlay-color"]?.present ||
    !summary.evidence.settingsInterfaceStyle.sliders?.["side-saturation"]?.present ||
    !summary.evidence.settingsInterfaceStyle.sliders?.["side-stroke-opacity"]?.present ||
    !summary.evidence.settingsInterfaceStyle.sliders?.["center-blur"]?.present ||
    !summary.evidence.settingsInterfaceStyle.sliders?.["center-opacity"]?.present ||
    !summary.evidence.settingsInterfaceStyle.sliders?.["center-overlay-strength"]?.present ||
    !summary.evidence.settingsInterfaceStyle.colors?.["center-overlay-color"]?.present ||
    !summary.evidence.settingsInterfaceStyle.sliders?.["center-saturation"]?.present ||
    !summary.evidence.settingsInterfaceStyle.sliders?.["center-stroke-opacity"]?.present ||
    !summary.evidence.settingsInterfaceStyle.sliders?.["dither-opacity"]?.present ||
    !summary.evidence.settingsInterfaceStyle.sliders?.["backdrop-gradient-strength"]?.present ||
    !summary.evidence.settingsInterfaceStyle.sliders?.["edge-highlight-brightness"]?.present ||
    !summary.evidence.settingsInterfaceStyle.sliders?.["halo-brightness"]?.present ||
    !summary.evidence.settingsInterfaceStyle.sliders?.["pointer-response-radius"]?.present ||
    !summary.evidence.settingsInterfaceStyle.sliders?.["pointer-sensing-distance"]?.present ||
    !summary.evidence.settingsInterfaceStyle.sliders?.["pointer-falloff"]?.present ||
    !summary.evidence.settingsInterfaceStyle.sliders?.["dialog-blur"]?.present ||
    !summary.evidence.settingsInterfaceStyle.sliders?.["dialog-opacity"]?.present ||
    !summary.evidence.settingsInterfaceStyle.sliders?.["dialog-overlay-strength"]?.present ||
    !summary.evidence.settingsInterfaceStyle.sliders?.["dialog-backdrop-dim"]?.present ||
    summary.evidence.settingsInterfaceStyle.sliders?.["window-corner-radius"]?.type !== "range" ||
    summary.evidence.settingsInterfaceStyle.sliders?.["surface-corner-radius"]?.type !== "range" ||
    summary.evidence.settingsInterfaceStyle.sliders?.["control-corner-radius"]?.type !== "range" ||
    summary.evidence.settingsInterfaceStyle.sliders?.["content-corner-radius"]?.type !== "range" ||
    summary.evidence.settingsInterfaceStyle.sliders?.["side-blur"]?.type !== "range" ||
    summary.evidence.settingsInterfaceStyle.sliders?.["side-opacity"]?.type !== "range" ||
    summary.evidence.settingsInterfaceStyle.sliders?.["side-overlay-strength"]?.type !== "range" ||
    summary.evidence.settingsInterfaceStyle.colors?.["side-overlay-color"]?.type !== "color" ||
    summary.evidence.settingsInterfaceStyle.sliders?.["side-saturation"]?.type !== "range" ||
    summary.evidence.settingsInterfaceStyle.sliders?.["side-stroke-opacity"]?.type !== "range" ||
    summary.evidence.settingsInterfaceStyle.sliders?.["center-blur"]?.type !== "range" ||
    summary.evidence.settingsInterfaceStyle.sliders?.["center-opacity"]?.type !== "range" ||
    summary.evidence.settingsInterfaceStyle.sliders?.["center-overlay-strength"]?.type !== "range" ||
    summary.evidence.settingsInterfaceStyle.colors?.["center-overlay-color"]?.type !== "color" ||
    summary.evidence.settingsInterfaceStyle.sliders?.["center-saturation"]?.type !== "range" ||
    summary.evidence.settingsInterfaceStyle.sliders?.["center-stroke-opacity"]?.type !== "range" ||
    summary.evidence.settingsInterfaceStyle.sliders?.["dither-opacity"]?.type !== "range" ||
    summary.evidence.settingsInterfaceStyle.sliders?.["backdrop-gradient-strength"]?.type !== "range" ||
    summary.evidence.settingsInterfaceStyle.sliders?.["edge-highlight-brightness"]?.type !== "range" ||
    summary.evidence.settingsInterfaceStyle.sliders?.["halo-brightness"]?.type !== "range" ||
    summary.evidence.settingsInterfaceStyle.sliders?.["pointer-response-radius"]?.type !== "range" ||
    summary.evidence.settingsInterfaceStyle.sliders?.["pointer-sensing-distance"]?.type !== "range" ||
    summary.evidence.settingsInterfaceStyle.sliders?.["pointer-falloff"]?.type !== "range" ||
    summary.evidence.settingsInterfaceStyle.sliders?.["dialog-blur"]?.type !== "range" ||
    summary.evidence.settingsInterfaceStyle.sliders?.["dialog-opacity"]?.type !== "range" ||
    summary.evidence.settingsInterfaceStyle.sliders?.["dialog-overlay-strength"]?.type !== "range" ||
    summary.evidence.settingsInterfaceStyle.sliders?.["dialog-backdrop-dim"]?.type !== "range"
  ) {
    hardFailures.push("Settings Interface style range sliders are missing or miswired");
  }
  if (!summary.evidence.settingsInterfaceStyle.resetButtonPresent) {
    hardFailures.push("Settings Interface style reset button is missing");
  }
  if (summary.evidence.settingsInterfaceStyle.edgeHighlightBrightness !== "6.5") {
    hardFailures.push("default glass edge highlight brightness is not 6.5 in Settings");
  }
  if (
    !summary.evidence.settingsInterfaceStyle.sliderChange?.changed ||
    summary.evidence.settingsInterfaceStyle.before?.sideBlur === summary.evidence.settingsInterfaceStyle.after?.sideBlur
  ) {
    hardFailures.push("Settings side blur slider did not change --glass-side-blur");
  }
  const appearanceMatrix = summary.evidence.settingsInterfaceStyle.appearanceMatrix;
  for (const [group, ids] of Object.entries(appearanceMatrix?.groups ?? {})) {
    for (const id of ids) {
      const initial = appearanceMatrix.initial?.sliders?.[id];
      const entry = appearanceMatrix.matrix?.[group]?.[id];
      const variableChanged = Object.keys(entry?.variables?.min ?? {}).some(
        (name) => entry.variables.min[name] !== entry.variables.max?.[name]
      );
      if (
        !initial ||
        !entry?.present ||
        entry.min !== initial.min ||
        entry.max !== initial.max ||
        entry.restored !== entry.default ||
        appearanceMatrix.final?.sliders?.[id]?.value !== entry.default ||
        !variableChanged
      ) {
        hardFailures.push(`Settings ${group} appearance matrix failed for ${id}`);
      }
    }
  }
  for (const surfaceName of ["titlebarLeft", "titlebarRight", "sidebar", "inspector"]) {
    const before = summary.evidence.settingsInterfaceStyle.materialBefore?.surfaces?.[surfaceName];
    const after = summary.evidence.settingsInterfaceStyle.materialAfter?.surfaces?.[surfaceName];
    if (!before?.present || !after?.present) {
      hardFailures.push(`material blur evidence missing ${surfaceName}`);
      continue;
    }
    if (!effectiveBlurFilter(before.backdropFilter) || !effectiveBlurFilter(after.backdropFilter)) {
      hardFailures.push(`${surfaceName} does not use an effective backdrop blur`);
    }
    if (before.backdropFilter === after.backdropFilter) {
      hardFailures.push(`${surfaceName} backdrop-filter did not change with blur slider`);
    }
  }
  const popoverSurface = summary.evidence.settingsInterfaceStyle.materialAfter?.surfaces?.popover;
  if (!popoverSurface?.present || !effectiveBlurFilter(popoverSurface.backdropFilter)) {
    hardFailures.push("floating popover does not use an effective frosted blur surface");
  }
  for (const value of Object.values(summary.evidence.settingsInterfaceStyle.materialAfter?.rootBackgrounds ?? {})) {
    if (!transparent(value)) hardFailures.push("root backgrounds changed away from transparent after blur slider update");
  }
  for (const value of Object.values(summary.evidence.settingsInterfaceStyle.materialAfter?.rootBackdropFilters ?? {})) {
    if (!noBackdropFilter(value)) hardFailures.push("root/app-shell backdrop-filter appeared after blur slider update");
  }
  if (
    !summary.evidence.notificationButtonStyle?.matchesCenter ||
    summary.evidence.notificationButtonStyle?.notification?.borderWidth !== "0px" ||
    summary.evidence.notificationButtonStyle?.notification?.afterBorderWidth !== "0px" ||
    summary.evidence.notificationButtonStyle?.notification?.pointerTarget !== "true"
  ) {
    hardFailures.push("notification button computed style does not match a center titlebar icon button");
  }
  for (const step of summary.evidence.compactPopovers.steps) {
    if (step.evidence.count > 1) {
      hardFailures.push(`multiple compact popovers remained open after ${step.action}`);
    }
    const expectedNames = step.expected === null ? [] : [step.expected];
    if (JSON.stringify(step.evidence.names) !== JSON.stringify(expectedNames)) {
      hardFailures.push(`compact popover state mismatch after ${step.action}`);
    }
    for (const root of step.evidence.roots) {
      if (!root.floating || !effectiveBlurFilter(root.backdropFilter)) {
        hardFailures.push(`${root.name} compact popover is not floating glass material`);
      }
      if (root.notification) {
        if (root.notification.activeProgressCount !== root.notification.activeCount) {
          hardFailures.push("notification compact active rows are missing progress tracks");
        }
        if (root.notification.thumbnailCount > root.notification.thumbnailLibraryCount) {
          hardFailures.push("notification compact rendered more thumbnail aggregates than available libraries");
        }
        if (!root.notification.onlyCurrentProgress) {
          hardFailures.push("notification compact rendered content outside current progress or status");
        }
        if (root.notification.actionCount !== 0) {
          hardFailures.push("notification compact exposed cancel/retry actions");
        }
        if (root.notification.fullCenterCount !== 1) {
          hardFailures.push("notification compact is missing the full-center action");
        }
        if (
          !root.notification.bodyScrollable ||
          !root.notification.footerVisible ||
          !root.notification.boundedWidth
        ) {
          hardFailures.push("notification compact body, footer, or bounded-width layout is invalid");
        }
        const tops = root.notification.contentOrder.map((item) => item.top);
        if (tops.some((top, index) => index > 0 && top < tops[index - 1])) {
          hardFailures.push("notification compact content sections are out of order");
        }
        if (
          root.rect.left < 0 ||
          root.rect.top < 0 ||
          root.rect.right > step.evidence.viewport.width ||
          root.rect.bottom > step.evidence.viewport.height
        ) {
          hardFailures.push("notification compact popover overflows the visual harness viewport");
        }
        if (
          !near(root.anchorGeometry?.gap, 6, 1) ||
          !root.anchorGeometry?.viewportContained ||
          (!root.anchorGeometry?.clampedLeft && !near(root.anchorGeometry?.rightDelta, 0, 1))
        ) {
          hardFailures.push("notification compact popover is not anchored to its titlebar button");
        }
      }
    }
    if (step.evidence.titlebar.notificationButtonCount !== 1) {
      hardFailures.push("titlebar does not expose exactly one notification button");
    }
    if (step.evidence.titlebar.legacyButtonCount !== 0) {
      hardFailures.push("legacy Tasks/Recent titlebar buttons are still present");
    }
    if (step.evidence.titlebar.visibleText.some((text) => /\d/.test(text))) {
      hardFailures.push("notification titlebar button exposes a numeric badge");
    }
  }
  const fixture = summary.evidence.notificationFixture;
  const fixtureRoot = (evidence) =>
    evidence?.roots?.find((root) => root.name === "notifications") ?? null;
  const fullFixtureRoot = fixtureRoot(fixture?.full);
  const narrowFixtureRoot = fixtureRoot(fixture?.narrow);
  const zoomedFixtureRoot = fixtureRoot(fixture?.zoomed);
  const zoomed150FixtureRoot = fixtureRoot(fixture?.zoomed150);
  const relocatedFixtureRoot = fixtureRoot(fixture?.relocated);
  const remountedFixtureRoot = fixtureRoot(fixture?.remounted);
  const failureFixtureRoot = fixtureRoot(fixture?.failure);
  const emptyFixtureRoot = fixtureRoot(fixture?.empty);
  const degradedFixtureRoot = fixtureRoot(fixture?.degraded);
  const validFixtureAnchor = (root) =>
    Boolean(
      root?.anchorGeometry?.viewportContained &&
        near(root.anchorGeometry.gap, 6, 1) &&
        (root.anchorGeometry.clampedLeft || near(root.anchorGeometry.rightDelta, 0, 1))
    );
  if (
    !fullFixtureRoot ||
    fullFixtureRoot.notification?.activeCount !== 2 ||
    fullFixtureRoot.notification?.activeProgressCount !== 2 ||
    fullFixtureRoot.notification?.determinateCount !== 1 ||
    fullFixtureRoot.notification?.indeterminateCount !== 1 ||
    fullFixtureRoot.notification?.phaseCount !== 2 ||
    fullFixtureRoot.notification?.progressLabelCount !== 2 ||
    fullFixtureRoot.notification?.progressPercentCount !== 1 ||
    fullFixtureRoot.notification?.thumbnailCount !== 2 ||
    fullFixtureRoot.notification?.thumbnailLibraryCount !== 2 ||
    JSON.stringify(fullFixtureRoot.notification?.thumbnailSummaries?.map((item) => item.libraryId)) !==
      JSON.stringify(["artists", "references"]) ||
    JSON.stringify(fullFixtureRoot.notification?.thumbnailSummaries?.map((item) => item.libraryName)) !==
      JSON.stringify(["Artists", "References"]) ||
    JSON.stringify(fullFixtureRoot.notification?.thumbnailSummaries?.map((item) => item.progress)) !==
      JSON.stringify(["82", "63"]) ||
    JSON.stringify(fullFixtureRoot.notification?.thumbnailSummaries?.map((item) => item.progressLabel)) !==
      JSON.stringify(["82%", "63%"]) ||
    !fullFixtureRoot.notification?.thumbnailSummaries?.[0]?.cachedTotal.includes("820") ||
    !fullFixtureRoot.notification?.thumbnailSummaries?.[0]?.cachedTotal.includes("1,000") ||
    !fullFixtureRoot.notification?.thumbnailSummaries?.[0]?.status.includes("32 待处理") ||
    !fullFixtureRoot.notification?.thumbnailSummaries?.[0]?.status.includes("4 失败") ||
    !fullFixtureRoot.notification?.thumbnailSummaries?.[0]?.status.includes("24 过期") ||
    !fullFixtureRoot.notification?.thumbnailSummaries?.[1]?.cachedTotal.includes("500") ||
    !fullFixtureRoot.notification?.thumbnailSummaries?.[1]?.cachedTotal.includes("800") ||
    !fullFixtureRoot.notification?.thumbnailSummaries?.[1]?.status.includes("18 待处理") ||
    !fullFixtureRoot.notification?.thumbnailSummaries?.[1]?.status.includes("9 失败") ||
    !fullFixtureRoot.notification?.thumbnailSummaries?.[1]?.status.includes("48 过期") ||
    !fullFixtureRoot.notification?.onlyCurrentProgress ||
    fullFixtureRoot.notification?.failure ||
    fullFixtureRoot.notification?.health !== "failure" ||
    !fullFixtureRoot.notification?.bodyScrollable ||
    !fullFixtureRoot.notification?.footerVisible ||
    !fullFixtureRoot.notification?.boundedWidth ||
    fullFixtureRoot.notification?.actionCount !== 0 ||
    fullFixtureRoot.notification?.fullCenterCount !== 1
  ) {
    hardFailures.push("full notification visual fixture does not cover the required compact content model");
  }
  if (
    !narrowFixtureRoot ||
    fixture?.narrow?.viewport?.width > 820 ||
    !narrowFixtureRoot.anchorGeometry?.viewportContained ||
    !near(narrowFixtureRoot.anchorGeometry?.gap, 6, 1) ||
    (!narrowFixtureRoot.anchorGeometry?.clampedLeft &&
      !near(narrowFixtureRoot.anchorGeometry?.rightDelta, 0, 1))
  ) {
    hardFailures.push("narrow notification visual fixture is detached from its anchor or overflows");
  }
  if (!validFixtureAnchor(zoomedFixtureRoot)) {
    hardFailures.push("zoomed notification visual fixture is detached from its anchor or overflows");
  }
  if (!validFixtureAnchor(zoomed150FixtureRoot)) {
    hardFailures.push("150 percent notification visual fixture is detached from its anchor or overflows");
  }
  const preferenceFixture = fixture?.preferences;
  if (
    preferenceFixture?.reducedMotion?.reducedMotion?.matches &&
    (preferenceFixture.reducedMotion.reducedMotion.animationName !== "none" ||
      preferenceFixture.reducedMotion.reducedMotion.transform !== "none")
  ) {
    hardFailures.push("reduced-motion notification fixture still animates its overlay surface");
  }
  if (
    preferenceFixture?.reducedTransparency?.reducedTransparency?.matches &&
    transparent(preferenceFixture.reducedTransparency.reducedTransparency.backgroundColor)
  ) {
    hardFailures.push("reduced-transparency notification fixture did not switch to an opaque surface");
  }
  if (!validFixtureAnchor(relocatedFixtureRoot) || !validFixtureAnchor(remountedFixtureRoot)) {
    hardFailures.push("relocated or remounted notification fixture lost its live anchor geometry");
  }
  const measurement = fixture?.measurement;
  if (
    !measurement ||
    measurement.pointerChurnAfter.popoverRectCalls !== measurement.pointerChurnBefore.popoverRectCalls ||
    measurement.detachedChurnAfter.popoverRectCalls !== measurement.detachedChurnBefore.popoverRectCalls ||
    measurement.detachedChurnAfter.detachedAnchorCount < 1
  ) {
    hardFailures.push("notification pointer or detached-node churn triggered redundant measurements");
  }
  if (
    !failureFixtureRoot?.notification?.failure ||
    failureFixtureRoot.notification.failureCue !== "部分任务失败，请打开完整通知中心查看" ||
    failureFixtureRoot.notification.health !== "failure" ||
    failureFixtureRoot.notification.healthLabel !== "需要关注" ||
    failureFixtureRoot.notification.empty ||
    failureFixtureRoot.notification.activeCount !== 0 ||
    failureFixtureRoot.notification.thumbnailCount !== 0 ||
    !failureFixtureRoot.notification.onlyCurrentProgress ||
    failureFixtureRoot.notification.fullCenterCount !== 1
  ) {
    hardFailures.push("failure-only notification visual fixture is missing its compact failure status");
  }
  if (
    !emptyFixtureRoot?.notification?.empty ||
    emptyFixtureRoot.notification.degraded ||
    emptyFixtureRoot.notification.health !== "healthy" ||
    emptyFixtureRoot.notification.failure ||
    emptyFixtureRoot.notification.thumbnailCount !== 0 ||
    emptyFixtureRoot.notification.thumbnailLibraryCount !== 0 ||
    emptyFixtureRoot.notification.thumbnailSummaries.length !== 0
  ) {
    hardFailures.push("empty notification visual fixture is missing its healthy empty state");
  }
  if (
    !degradedFixtureRoot?.notification?.empty ||
    !degradedFixtureRoot.notification.degraded ||
    degradedFixtureRoot.notification.health !== "degraded" ||
    degradedFixtureRoot.notification.failure
  ) {
    hardFailures.push("degraded notification visual fixture is missing its degraded empty state");
  }
  const unexpectedFixtureConsoleErrors =
    fixture?.navigationDiagnostics?.consoleErrors?.filter(
      (entry) =>
        !/sandboxed_renderer\.bundle\.js script failed to run|preloadScripts.*startupData/i.test(
          entry.text ?? ""
        )
    ) ?? [];
  const unexpectedFixtureNetworkProblems =
    fixture?.navigationDiagnostics?.networkProblems?.filter(
      (entry) => entry.errorText !== "net::ERR_ABORTED"
    ) ?? [];
  if (unexpectedFixtureConsoleErrors.length || unexpectedFixtureNetworkProblems.length) {
    hardFailures.push("notification test renderer emitted unexpected console or network failures");
  }
  const fullWindowFixture = summary.evidence.notificationWindowFixture;
  if (
    fullWindowFixture?.status?.tabCount !== 3 ||
    fullWindowFixture.status.status?.active !== 2 ||
    fullWindowFixture.status.status?.cancelActions !== 1 ||
    fullWindowFixture.status.status?.indeterminate !== 1 ||
    fullWindowFixture.status.status?.thumbnails !== 2 ||
    fullWindowFixture.status.status?.results !== 3 ||
    !fullWindowFixture.status.bodyBackground?.includes("gradient") ||
    fullWindowFixture.status.navBorderRightWidth !== "0px" ||
    fullWindowFixture.status.contentOverflow !== "hidden" ||
    fullWindowFixture.status.titlebarBorderBottomWidth !== "1px"
  ) {
    hardFailures.push("full notification window fixture is missing status/material/no-divider coverage");
  }
  if (
    !fullWindowFixture?.historyInitial?.history?.present ||
    fullWindowFixture.historyInitial.history.filterCount < 5 ||
    fullWindowFixture.historyInitial.history.renderedRows >= 100 ||
    !fullWindowFixture.history?.history?.actionError?.includes("Fixture retry rejected") ||
    !fullWindowFixture.history?.history?.footer?.includes("200")
  ) {
    hardFailures.push("full notification window fixture is missing history paging/action coverage");
  }
  if (
    !fullWindowFixture?.logsGap?.logs?.gap ||
    !fullWindowFixture.logsGap.logs.controls.pause ||
    !fullWindowFixture.logsGap.logs.controls.follow ||
    !fullWindowFixture.logsGap.logs.controls.copy ||
    !fullWindowFixture.logsGap.logs.controls.clear ||
    !fullWindowFixture.logsGap.logs.controls.export ||
    fullWindowFixture.logsGap.logs.renderedLines >= 100 ||
    !fullWindowFixture.logs?.logs?.actionMessage?.includes("已清空") ||
    fullWindowFixture.cleanup?.metrics?.activeLogSubscriptions !== 0 ||
    fullWindowFixture.cleanup?.metrics?.logUnsubscriptions < 1
  ) {
    hardFailures.push("full notification window fixture is missing bounded logs/actions/tab-cleanup coverage");
  }
  if (
    fullWindowFixture?.narrow?.viewport?.width > 660 ||
    fullWindowFixture?.narrow?.navBorderRightWidth !== "0px" ||
    !fullWindowFixture?.degraded?.status?.degraded
  ) {
    hardFailures.push("full notification window fixture is missing narrow/degraded coverage");
  }
  const notificationMaximizeEvidence = summary.evidence.notificationMaximize;
  if (
    notificationMaximizeEvidence?.bridgeAvailable &&
    (!notificationMaximizeEvidence.maximizedState?.matched ||
      !notificationMaximizeEvidence.restoredState?.matched ||
      !validFixtureAnchor(notificationMaximizeEvidence.before?.roots?.find((root) => root.name === "notifications")) ||
      !validFixtureAnchor(notificationMaximizeEvidence.maximized?.roots?.find((root) => root.name === "notifications")) ||
      !validFixtureAnchor(notificationMaximizeEvidence.restored?.roots?.find((root) => root.name === "notifications")))
  ) {
    hardFailures.push("notification anchor geometry did not survive maximize and restore");
  }
  if (inspectorPortrait.centralOpen) hardFailures.push("central preview open during right preview capture");
  if (inspectorPortrait.headingPresent) hardFailures.push("right preview still renders the old heading row");
  if (inspectorPortrait.source !== "thumbnail") hardFailures.push("right preview is not using thumbnail source");
  if (summary.evidence.inspectorThumbnailBlobResponsesBeforeCentralOpen.length === 0) {
    hardFailures.push("right preview did not stay on the light grid_320 thumbnail path");
  }
  if (!transparent(inspectorPortrait.stageBackground) || !transparent(inspectorPortrait.readyBackground)) {
    hardFailures.push("right preview still has opaque fill");
  }
  if (!/^0px none/.test(inspectorPortrait.stageBorder ?? "")) hardFailures.push("right preview still has a border");
  if (inspectorPortrait.stageBoxShadow !== "none") {
    hardFailures.push("right preview stage still has a frame-like shadow");
  }
  if (!zeroCssLengths(inspectorPortrait.stagePadding)) hardFailures.push("right preview stage still has padding");
  if (!near(inspectorPortrait.stageHeight, inspectorPortrait.stageRect?.width, 2)) {
    hardFailures.push("right preview stage does not remain square within the inspector width");
  }
  if (inspectorPortrait.imageObjectFit !== "contain") hardFailures.push("right preview media is not using contain object fit");
  if (!nonZeroRadius(inspectorPortrait.imageBorderRadius)) hardFailures.push("right preview media does not retain rounded corners");
  if (!inspectorPortrait.imageWithinStage) hardFailures.push("portrait right preview image is cropped outside its stage");
  if (inspectorPortrait.imageCenterDelta?.x > 2 || inspectorPortrait.imageCenterDelta?.y > 2) {
    hardFailures.push("portrait right preview image is not centered in its stage");
  }
  if (landscapeFit.mode !== "fit-long-edge") hardFailures.push("landscape default preview mode is not fit-long-edge");
  if (!transparent(landscapeFit.stageBackground) || !transparent(landscapeFit.readyBackground)) {
    hardFailures.push("landscape central preview has opaque backing");
  }
  if (!/^0px none/.test(landscapeFit.stageBorder ?? "")) hardFailures.push("landscape central preview stage still has a border");
  if (!zeroCssLengths(landscapeFit.stageBorderWidth) || !zeroCssLengths(landscapeFit.stagePadding)) {
    hardFailures.push("landscape central preview stage still has border width or padding");
  }
  if (landscapeFit.stageBoxShadow !== "none") {
    hardFailures.push("landscape central preview stage still has a frame-like shadow");
  }
  if (!zeroCssLengths(landscapeFit.readyPadding)) {
    hardFailures.push("landscape central preview ready media wrapper still has padding");
  }
  if (!(landscapeFit.titleCenterDelta <= 2)) {
    hardFailures.push("landscape opened image title is not centered in the titlebar");
  }
  if (hasInsetFocusRing(landscapeFit.stageBoxShadow)) {
    hardFailures.push("landscape central preview stage still has an inset focus ring");
  }
  if (hasVisibleOutline(landscapeFit.stageOutlineStyle, landscapeFit.stageOutlineWidth)) {
    hardFailures.push("landscape central preview stage still has an outline ring");
  }
  if (!near(landscapeFit.actualScale, landscapeFit.expectedFitScale, 0.015)) {
    hardFailures.push("landscape default preview is not scaled to fit long edge");
  }
  if (landscapeFit.imageObjectFit !== "contain") hardFailures.push("landscape central preview media is not using contain object fit");
  if (!rectWithin(landscapeFit.imageRect, landscapeFit.stageRect)) {
    hardFailures.push("landscape default preview image is outside the stage bounds");
  }
  if (landscapeActual.mode !== "actual" || !near(landscapeActual.actualScale, 1, 0.015)) {
    hardFailures.push("double-click did not switch landscape preview to 100%");
  }
  if (landscapeFitAgain.mode !== "fit-long-edge" || !near(landscapeFitAgain.actualScale, landscapeFitAgain.expectedFitScale, 0.015)) {
    hardFailures.push("second double-click did not return to fit-long-edge");
  }
  if (!rectWithin(landscapeFitAgain.imageRect, landscapeFitAgain.stageRect)) {
    hardFailures.push("landscape fit-long-edge preview after toggle is outside the stage bounds");
  }
  if (
    landscapeToActualAnchor.anchorBefore &&
    landscapeToActualAnchor.anchorAfter &&
    (!near(landscapeToActualAnchor.anchorBefore.x, landscapeToActualAnchor.anchorAfter.x, 0.08) ||
      !near(landscapeToActualAnchor.anchorBefore.y, landscapeToActualAnchor.anchorAfter.y, 0.08))
  ) {
    hardFailures.push("double-click actual zoom did not preserve the mouse anchor");
  }
  if (portraitFit.mode !== "fit-long-edge") hardFailures.push("portrait default preview mode is not fit-long-edge");
  if (!transparent(portraitFit.stageBackground) || !transparent(portraitFit.readyBackground)) {
    hardFailures.push("portrait central preview has opaque backing");
  }
  if (!/^0px none/.test(portraitFit.stageBorder ?? "")) hardFailures.push("portrait central preview stage still has a border");
  if (!zeroCssLengths(portraitFit.stageBorderWidth) || !zeroCssLengths(portraitFit.stagePadding)) {
    hardFailures.push("portrait central preview stage still has border width or padding");
  }
  if (portraitFit.stageBoxShadow !== "none") {
    hardFailures.push("portrait central preview stage still has a frame-like shadow");
  }
  if (!zeroCssLengths(portraitFit.readyPadding)) {
    hardFailures.push("portrait central preview ready media wrapper still has padding");
  }
  if (!(portraitFit.titleCenterDelta <= 2)) {
    hardFailures.push("portrait opened image title is not centered in the titlebar");
  }
  if (hasInsetFocusRing(portraitFit.stageBoxShadow)) {
    hardFailures.push("portrait central preview stage still has an inset focus ring");
  }
  if (hasVisibleOutline(portraitFit.stageOutlineStyle, portraitFit.stageOutlineWidth)) {
    hardFailures.push("portrait central preview stage still has an outline ring");
  }
  if (!near(portraitFit.actualScale, portraitFit.expectedFitScale, 0.015)) {
    hardFailures.push("portrait default preview is not scaled to fit long edge");
  }
  if (portraitFit.imageObjectFit !== "contain") hardFailures.push("portrait central preview media is not using contain object fit");
  if (!rectWithin(portraitFit.imageRect, portraitFit.stageRect)) {
    hardFailures.push("portrait central preview image is outside the stage bounds");
  }
  if (pointerNearEdge.edgeHighlightBrightness !== "6.5") {
    hardFailures.push("root glass edge highlight brightness is not 6.5 during pointer evidence");
  }
  if (pointerNearEdge.haloBrightness !== "") {
    hardFailures.push("retired root surface halo variable was restored");
  }
  if (
    pointerFarIdle.titlebarLeft.opacity !== 0 ||
    pointerFarIdle.titlebarCenter.opacity !== 0 ||
    pointerFarIdle.titlebarRight.opacity !== 0 ||
    pointerFarIdle.inspector.opacity !== 0 ||
    pointerFarIdle.sidebar.opacity !== 0
  ) {
    hardFailures.push("retired structural surface pointer highlight reactivated");
  }
  if (
    pointerWindowOuterEdge.windowEdge.legacyFramePresent ||
    pointerWindowOuterEdge.windowEdge.borderRadius === "0px" ||
    !["hidden", "clip"].includes(pointerWindowOuterEdge.windowEdge.overflow) ||
    pointerWindowOuterEdge.windowEdge.borderWidth !== "0px"
  ) {
    hardFailures.push("window shell lost its clean rounded clip or restored the legacy edge frame");
  }
  if (
    pointerNearEdge.titlebarLeft.opacity !== 0 ||
    pointerNearEdge.titlebarCenter.opacity !== 0 ||
    pointerNearEdge.sidebar.opacity !== 0
  ) {
    hardFailures.push("unified pointer highlight leaked onto structural glass surfaces");
  }
  if (
    pointerTreeItem.treeItem.targetAttr !== "true" ||
    !(pointerTreeItem.treeItem.interactiveOpacity > 0)
  ) {
    hardFailures.push("folder tree items do not participate in the unified pointer highlight");
  }
  if (!pointerTracksPoint(pointerTreeItem.treeItem, summary.evidence.pointerPoints.treeItem)) {
    hardFailures.push("folder tree item pointer glow does not follow the real cursor in local pixel space");
  }
  if (
    pointerTreeItem.treeLabelButton.targetAttr === "true" ||
    pointerTreeItem.treeDisclosureButton.targetAttr === "true"
  ) {
    hardFailures.push("folder tree inner buttons are drawing their own local edge highlight instead of the tree item owning the stroke");
  }
  if (
    pointerTileThumb.tileThumb.targetAttr !== "true" ||
    !(pointerTileThumb.tileThumb.interactiveOpacity > 0)
  ) {
    hardFailures.push("grid thumbnails do not participate in the unified pointer highlight");
  }
  if (!pointerTracksPoint(pointerTileThumb.tileThumb, summary.evidence.pointerPoints.tileThumb)) {
    hardFailures.push("grid thumbnail pointer glow does not follow the real cursor in local pixel space");
  }
  if (
    !/radial-gradient/i.test(pointerTileThumb.tileThumb.beforeBackgroundImage ?? "") ||
    !/radial-gradient/i.test(pointerTileThumb.tileThumb.afterBackgroundImage ?? "") ||
    !/linear-gradient/i.test(
      pointerTileThumb.tileThumb.afterMaskImage ??
        pointerTileThumb.tileThumb.afterWebkitMaskImage ??
        ""
    )
  ) {
    hardFailures.push("interactive pointer fill and masked border are not rendered around the real cursor");
  }
  const pointerSettingsSlider = summary.evidence.settingsInterfaceStyle.pointerSlider;
  if (
    pointerSettingsSlider?.settingsSliderControl?.targetAttr !== "true" ||
    !(pointerSettingsSlider?.settingsSliderControl?.interactiveOpacity > 0) ||
    pointerSettingsSlider?.settingsSliderInput?.targetAttr === "true"
  ) {
    hardFailures.push("settings range sliders do not expose exactly one unified pointer-highlight owner");
  }
  if (!pointerTracksPoint(
    pointerSettingsSlider?.settingsSliderControl,
    summary.evidence.settingsInterfaceStyle.pointerPoint
  )) {
    hardFailures.push("settings range slider pointer glow does not follow the real cursor in local pixel space");
  }
  if (summary.evidence.previewApiResponses.length === 0) {
    hardFailures.push("preview route was not requested");
  }

  summary = { ...summary, hardFailures, fatalError: null };
  await writeFile(summaryPath, JSON.stringify(summary, null, 2));

  if (hardFailures.length) {
    throw new Error(`Desktop UI visual regression failed: ${hardFailures.join(", ")}. See ${summaryPath}`);
  }
  }
} catch (error) {
  fatalError = error;
  if (!summary) {
    summary = {
      run: {
        id: visualRunId,
        startedAtUtc: runStartedAt.toISOString(),
        completedAtUtc: new Date().toISOString()
      },
      screenshots: {},
      evidence: {},
      consoleWarnings,
      consoleErrors,
      networkProblems,
      startupProblems: startupWarningsAndErrors(),
      stdoutPath,
      stderrPath,
      hardFailures,
      fatalError: serializeError(error)
    };
  } else {
    summary = {
      ...summary,
      startupProblems: startupWarningsAndErrors(),
      hardFailures,
      fatalError: serializeError(error)
    };
  }
  await writeFile(summaryPath, JSON.stringify(summary, null, 2));
  throw error;
} finally {
  if (summary) await writeFile(summaryPath, JSON.stringify(summary, null, 2));
  if (client) client.close();
  await stopDevApp();
  stdout.end();
  stderr.end();
  await delay(300);
}
} finally {
  if (!fatalError) {
    console.log(`Desktop UI visual regression complete. Summary: ${summaryPath}`);
  }
}
