import { spawn } from "node:child_process";
import { createWriteStream } from "node:fs";
import { mkdir, readdir, readFile, rm, writeFile } from "node:fs/promises";
import http from "node:http";
import path from "node:path";
import process from "node:process";
import WebSocket from "ws";
import { inspectNativeBrowserWindowOptions } from "../../tools/checks/native-browser-window-options.mjs";

const root = "D:\\Megle";
const visualRoot = path.join(root, ".tmp", "visual-check");
const mediaDir = path.join(visualRoot, "media");
const screenshotDir = path.join(visualRoot, "screenshots");
const logDir = path.join(visualRoot, "logs");
const dataDir = path.join(visualRoot, "data-desktop-ui");
const electronUserDataDir = path.join(dataDir, "electron-user-data");
const webUrl = "http://127.0.0.1:5179";
const debugPort = Number(process.env.MEGLE_VISUAL_DEBUG_PORT ?? process.env.MEGLE_REMOTE_DEBUG_PORT ?? 9239);
const visualRunId = `megle-visual-${Date.now().toString(36)}-${process.pid}`;
const runStartedAt = new Date();
const osBackdropEvidenceEnabled = process.env.MEGLE_VISUAL_OS_BACKDROP === "1";
const osBackdropEvidenceMode = osBackdropEvidenceEnabled
  ? "required-os-composited"
  : "optional-skipped-static-only";
const osBackdropRequiredCommand =
  '$env:MEGLE_VISUAL_OS_BACKDROP="1"; node .tmp\\visual-check\\desktop-ui-regression.mjs; Remove-Item Env:\\MEGLE_VISUAL_OS_BACKDROP';

const consoleWarnings = [];
const consoleErrors = [];
const networkProblems = [];
const responses = [];
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

await rm(dataDir, { recursive: true, force: true });
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
[MegleBackdropCapture]::ShowWindow($window.MainWindowHandle, 9) | Out-Null
[MegleBackdropCapture]::SetForegroundWindow($window.MainWindowHandle) | Out-Null
[MegleBackdropCapture]::SetWindowPos($window.MainWindowHandle, $topMost, 0, 0, 0, 0, ($swpNoMove -bor $swpNoSize -bor $swpShowWindow)) | Out-Null
Start-Sleep -Milliseconds 220
$rect = New-Object MegleBackdropCapture+RECT
if (-not [MegleBackdropCapture]::GetWindowRect($window.MainWindowHandle, [ref] $rect)) {
  throw 'GetWindowRect failed before backdrop capture'
}
$windowWidth = [Math]::Max(1, $rect.Right - $rect.Left)
$windowHeight = [Math]::Max(1, $rect.Bottom - $rect.Top)
if ($rect.Left -lt ($margin + 12) -or $rect.Top -lt ($margin + 12)) {
  [MegleBackdropCapture]::MoveWindow($window.MainWindowHandle, 72, 72, $windowWidth, $windowHeight, $true) | Out-Null
  Start-Sleep -Milliseconds 260
  if (-not [MegleBackdropCapture]::GetWindowRect($window.MainWindowHandle, [ref] $rect)) {
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
[MegleBackdropCapture]::SetWindowPos($window.MainWindowHandle, $topMost, 0, 0, 0, 0, ($swpNoMove -bor $swpNoSize -bor $swpShowWindow)) | Out-Null
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
      handle = $window.MainWindowHandle.ToInt64()
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
  [MegleBackdropCapture]::SetWindowPos($window.MainWindowHandle, $notTopMost, 0, 0, 0, 0, ($swpNoMove -bor $swpNoSize -bor $swpShowWindow)) | Out-Null
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

async function doubleClickTitlebarPoint(client, point) {
  const viewport = await evaluate(
    client,
    `({ width: window.innerWidth, height: window.innerHeight, devicePixelRatio: window.devicePixelRatio })`
  );
  await doubleClickPoint(client, point);
  return { inputMethod: "cdp", viewport };
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

  const initial = await desktopMaximizedState(client);
  if (initial === true) {
    await evaluate(client, `window.megleDesktop.windowControls.maximize()`);
    await waitForMaximizedState(client, false);
  }
  const before = await desktopMaximizedState(client);
  const point = await titlebarCenterBlankPoint(client);
  const firstInput = await doubleClickTitlebarPoint(client, point);
  const afterFirst = await waitForMaximizedState(client, true);
  const restorePoint = await titlebarCenterBlankPoint(client);
  const secondInput = await doubleClickTitlebarPoint(client, restorePoint);
  const afterSecond = await waitForMaximizedState(client, false);
  return {
    bridgeAvailable: true,
    bridge,
    initial,
    before,
    point,
    restorePoint,
    firstInput,
    secondInput,
    afterFirst,
    afterSecond
  };
}

async function searchNoDragEvidence(client) {
  await clickSelector(client, ".shell-titlebar-center .search-bar-input");
  return evaluate(
    client,
    `(() => {
      const input = document.querySelector(".shell-titlebar-center .search-bar-input");
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
      const roots = [...document.querySelectorAll("[data-compact-popover-root]")];
      if (${expectedValue} === null) return roots.length === 0;
      return roots.length === 1 && roots[0].getAttribute("data-compact-popover-root") === ${expectedValue};
    })()`,
    timeoutMs,
    expected === null ? "compact popovers closed" : `${expected} compact popover`
  );
}

async function compactPopoverDismissalEvidence(client) {
  const steps = [];

  await pressKey(client, "Escape", "Escape", 27);
  await waitForCompactPopoverState(client, null).catch(() => undefined);

  for (const name of ["tasks", "recent", "filter", "sort"]) {
    await clickSelector(client, `[data-compact-popover-trigger="${name}"]`);
    await waitForCompactPopoverState(client, name);
    steps.push({
      action: `open-${name}`,
      expected: name,
      evidence: await evaluate(client, compactPopoverEvidenceExpression())
    });
  }

  await pressKey(client, "Escape", "Escape", 27);
  await waitForCompactPopoverState(client, null);
  steps.push({
    action: "escape-closes-sort",
    expected: null,
    evidence: await evaluate(client, compactPopoverEvidenceExpression())
  });

  await clickSelector(client, `[data-compact-popover-trigger="tasks"]`);
  await waitForCompactPopoverState(client, "tasks");
  steps.push({
    action: "open-tasks-before-workspace-click",
    expected: "tasks",
    evidence: await evaluate(client, compactPopoverEvidenceExpression())
  });

  await clickSelector(client, ".grid-surface", 0.5, 0.5);
  await waitForCompactPopoverState(client, null);
  steps.push({
    action: "workspace-click-closes-tasks",
    expected: null,
    evidence: await evaluate(client, compactPopoverEvidenceExpression())
  });

  return { steps };
}

async function glassMaterialEvidence(client) {
  await pressKey(client, "Escape", "Escape", 27);
  await waitForCompactPopoverState(client, null).catch(() => undefined);
  await clickSelector(client, `[data-compact-popover-trigger="tasks"]`);
  await waitForCompactPopoverState(client, "tasks");
  const evidence = await evaluate(client, glassMaterialEvidenceExpression());
  await pressKey(client, "Escape", "Escape", 27);
  await waitForCompactPopoverState(client, null);
  return evidence;
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
        controls: Object.fromEntries(
          ["library-switcher", "toggle-sidebars", "open-plugins", "open-settings"]
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
      `({
        role: window.megleDesktop?.windowRole,
        title: document.querySelector(".secondary-window-title")?.textContent?.trim(),
        closePresent: Boolean(document.querySelector(".secondary-window-close")),
        pluginSurfacePresent: Boolean(document.querySelector(".plugins-layout"))
      })`
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
      const backdrop = pseudo ? null : element?.querySelector(".liquid-glass-backdrop") ?? null;
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
    const natural = image ? { width: image.naturalWidth, height: image.naturalHeight } : null;
    const expectedFitScale = stageRect && natural
      ? Math.min(stageRect.width / natural.width, stageRect.height / natural.height)
      : null;
    const actualScale = imageRect && natural ? imageRect.width / natural.width : null;
    return {
      mode: stage?.getAttribute("data-preview-mode") ?? null,
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
      const backdrop = element?.querySelector(".liquid-glass-backdrop") ?? null;
      const backdropStyle = backdrop ? getComputedStyle(backdrop) : null;
      return {
        present: Boolean(element),
        className: element?.className?.toString?.() ?? null,
        backgroundColor: style?.backgroundColor ?? null,
        backgroundImage: style?.backgroundImage ?? null,
        backdropLayerPresent: Boolean(backdrop),
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
        popover: surface("[data-compact-popover-root]")
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
        dataset: element.getAttribute("data-glass-pointer"),
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
    const searchInput = document.querySelector(".shell-titlebar-center .search-bar-input");
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
        dataset: element.getAttribute("data-glass-pointer"),
        glassPointerOpacity: Number(style.getPropertyValue("--glass-pointer-opacity") || 0),
        glassIllumination: Number(style.getPropertyValue("--glass-illumination") || 0),
        glassLensOpacity: Number(style.getPropertyValue("--glass-lens-opacity") || 0)
      };
    });
    return { controls };
  })()`;
}

function compactPopoverEvidenceExpression() {
  return `(() => {
    const roots = [...document.querySelectorAll("[data-compact-popover-root]")].map((element) => {
      const style = getComputedStyle(element);
      const backdrop = element.querySelector(".liquid-glass-backdrop");
      const backdropStyle = backdrop ? getComputedStyle(backdrop) : null;
      const rect = element.getBoundingClientRect();
      return {
        name: element.getAttribute("data-compact-popover-root"),
        className: element.className?.toString?.() ?? null,
        floating: element.classList.contains("floating-popover"),
        backgroundColor: style.backgroundColor,
        backgroundImage: style.backgroundImage,
        backdropLayerPresent: Boolean(backdrop),
        backdropFilter: backdropStyle?.backdropFilter ?? null,
        webkitBackdropFilter: backdropStyle?.webkitBackdropFilter ?? null,
        boxShadow: style.boxShadow,
        border: style.border,
        rect: { x: rect.x, y: rect.y, width: rect.width, height: rect.height, top: rect.top, right: rect.right, bottom: rect.bottom, left: rect.left }
      };
    });
    return {
      count: roots.length,
      names: roots.map((root) => root.name),
      roots,
      taskCenterOpen: Boolean(document.querySelector(".task-center-overlay"))
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
      return {
        selector,
        targetAttr: element?.getAttribute("data-interactive-pointer-target") ?? null,
        interactiveDataset: element?.getAttribute("data-interactive-pointer") ?? null,
        interactiveOpacity: Number(style?.getPropertyValue("--interactive-pointer-opacity") || 0),
        interactiveX: style?.getPropertyValue("--interactive-pointer-x").trim() ?? null,
        interactiveY: style?.getPropertyValue("--interactive-pointer-y").trim() ?? null,
        beforeBackgroundImage: before?.backgroundImage ?? null,
        beforeBackgroundBlendMode: before?.backgroundBlendMode ?? null,
        beforeMaskImage: before?.maskImage ?? null,
        beforeWebkitMaskImage: before?.webkitMaskImage ?? null,
        afterBackgroundImage: after?.backgroundImage ?? null
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
  client.on("Network.loadingFailed", (event) => {
    if (event.type !== "WebSocket") {
      networkProblems.push({ requestId: event.requestId, status: "loadingFailed", type: event.type, errorText: event.errorText });
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
    ".shell-titlebar-center .titlebar-icon-button, .shell-titlebar-center .filter-menu-trigger, .shell-titlebar-center .sort-menu-trigger";
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
  const searchNoDrag = await searchNoDragEvidence(client);
  const settingsInterfaceStyle = await settingsInterfaceStyleEvidence(client);
  const pluginsSecondaryWindow = await pluginsSecondaryWindowEvidence(client);
  const compactPopovers = await compactPopoverDismissalEvidence(client);

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
  await waitFor(
    client,
    `(() => {
      const image = document.querySelector(".central-preview-stage .preview-image");
      return Boolean(image && image.complete && image.naturalWidth > 0 && image.naturalHeight > 0);
    })()`,
    30000,
    "landscape central preview image"
  );
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
      const image = document.querySelector(".central-preview-stage .preview-image");
      return Boolean(image && image.complete && image.naturalWidth > 0 && image.naturalHeight > 0);
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
      selectedPortraitRightPreview,
      centralLandscapeFit,
      centralLandscapeActual,
      centralPortraitFit,
      pointerFarIdle: pointerFarIdleScreenshot,
      pointerWindowOuterEdge: pointerWindowOuterEdgeScreenshot,
      localEdgeHighlight,
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
      searchNoDrag,
      settingsInterfaceStyle: settingsInterfaceStyle.evidence,
      pluginsSecondaryWindow,
      compactPopovers,
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
  const unknownStartupProblems = summary.startupProblems.filter((problem) => !problem.known);
  if (consoleErrors.length) hardFailures.push("console errors");
  if (networkProblems.length) hardFailures.push("network failures");
  if (unknownConsoleWarnings.length) hardFailures.push("unknown console warnings");
  if (unknownStartupProblems.length) hardFailures.push("unknown startup warnings/errors");
  if (!transparent(layout.htmlBackground) || !transparent(layout.bodyBackground) || !transparent(layout.rootBackground)) {
    hardFailures.push("html/body/root backgrounds are not transparent");
  }
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
  const activeTitlebarToolGlass = summary.evidence.titlebarToolGlassActive.controls?.find((control) =>
    control.dataset === "active" || control.glassPointerOpacity > 0
  );
  if (!activeTitlebarToolGlass) {
    hardFailures.push("middle titlebar tool hover did not activate any liquid-glass control");
  } else {
    const backdrop = activeTitlebarToolGlass.layers?.backdrop ?? {};
    const lens = activeTitlebarToolGlass.layers?.lens ?? {};
    const edge = activeTitlebarToolGlass.layers?.edge ?? {};
    if (
      activeTitlebarToolGlass.glassPointerOpacity <= 0 ||
      activeTitlebarToolGlass.glassIllumination <= 0 ||
      activeTitlebarToolGlass.glassLensOpacity <= 0
    ) {
      hardFailures.push("middle titlebar tool hover did not set liquid-glass pointer/lens variables");
    }
    if (
      backdrop.opacity !== "1" ||
      !effectiveBlurFilter(backdrop.backdropFilter) ||
      !/megle-liquid-glass-refraction/.test(backdrop.filter ?? "")
    ) {
      hardFailures.push("middle titlebar tool hover did not restore backdrop blur plus SVG refraction");
    }
    if (!lens.present || Number(lens.opacity) <= 0 || !/megle-liquid-glass-edge/.test(lens.filter ?? "")) {
      hardFailures.push("middle titlebar tool hover did not keep the SVG lens filter active");
    }
    if (!edge.present || Number(edge.opacity) <= 0) {
      hardFailures.push("middle titlebar tool hover did not keep the local edge highlight active");
    }
  }
  const nearbyTitlebarToolGlass = summary.evidence.titlebarToolGlassNearby.controls?.find((control) =>
    control.dataset === "active" || control.glassPointerOpacity > 0
  );
  if (!nearbyTitlebarToolGlass) {
    hardFailures.push("titlebar buttons did not refresh local edge highlight from nearby pointer movement inside the titlebar");
  }
  const nearbyAnyTitlebarButton = summary.evidence.titlebarAllButtonsNearby.controls?.find((control) =>
    control.dataset === "active" || control.glassPointerOpacity > 0
  );
  if (!nearbyAnyTitlebarButton) {
    hardFailures.push("titlebar right-side buttons did not refresh from nearby pointer movement inside the titlebar");
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
  } else if (
    summary.evidence.titlebarDrag.before !== false ||
    !summary.evidence.titlebarDrag.afterFirst?.matched ||
    summary.evidence.titlebarDrag.afterFirst?.value !== true ||
    !summary.evidence.titlebarDrag.afterSecond?.matched ||
    summary.evidence.titlebarDrag.afterSecond?.value !== false
  ) {
    hardFailures.push("double-clicking blank center titlebar space did not maximize and restore the window");
  }
  if (summary.evidence.titlebarDrag.point?.titlebarAppRegion !== "no-drag") {
    hardFailures.push("center titlebar surface must remain pointer-addressable for local highlight and custom drag");
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
    JSON.stringify(["library-switcher", "toggle-sidebars", "open-plugins", "open-settings"])
  ) {
    hardFailures.push("left titlebar library/sidebar/Plugins/Settings control order is incorrect");
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
    !pluginsWindowEvidence.pluginWindow?.pluginSurfacePresent
  ) {
    hardFailures.push("Plugins window single-instance/coexistence/renderer-role contract failed");
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
    }
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
  if (inspectorPortrait.stageHeight < 250 || inspectorPortrait.stageHeight > 270) {
    hardFailures.push("right preview stage is not fixed around 260px");
  }
  if (!zeroRadius(inspectorPortrait.stageBorderRadius)) hardFailures.push("right preview stage border radius is not 0");
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
  if (pointerNearEdge.haloBrightness !== "1.45") {
    hardFailures.push("root halo brightness is not 1.45 during pointer evidence");
  }
  if (pointerFarIdle.inspector.opacity !== 0 || pointerFarIdle.inspector.dataset !== "idle") {
    hardFailures.push("glass edge highlight activates far from an edge");
  }
  if (
    pointerWindowOuterEdge.windowEdge.legacyFramePresent ||
    pointerWindowOuterEdge.windowEdge.borderRadius === "0px" ||
    !["hidden", "clip"].includes(pointerWindowOuterEdge.windowEdge.overflow) ||
    pointerWindowOuterEdge.windowEdge.borderWidth !== "0px"
  ) {
    hardFailures.push("window shell lost its clean rounded clip or restored the legacy edge frame");
  }
  if (!(pointerNearEdge.inspector.opacity > 0) || pointerNearEdge.inspector.dataset !== "active") {
    hardFailures.push("glass edge highlight does not activate near the local edge");
  }
  if (!nearCssPercent(pointerNearEdge.inspector.y, 98.6, 2)) {
    hardFailures.push("glass edge highlight position is snapped to the pane edge instead of following the real cursor");
  }
  if (
    pointerNearEdge.titlebarLeft.opacity !== 0 ||
    pointerNearEdge.titlebarCenter.opacity !== 0 ||
    pointerNearEdge.sidebar.opacity !== 0
  ) {
    hardFailures.push("edge highlight leaked to distant glass surfaces");
  }
  if (!(pointerTreeItem.treeItem.interactiveOpacity > 0) || pointerTreeItem.treeItem.interactiveDataset !== "active") {
    hardFailures.push("folder tree items do not participate in the shared local edge highlight model");
  }
  if (
    !nearCssPercent(pointerTreeItem.treeItem.interactiveX, 35, 2) ||
    !nearCssPercent(pointerTreeItem.treeItem.interactiveY, 60, 2)
  ) {
    hardFailures.push("folder tree item pointer glow is snapped to an edge instead of following the real cursor");
  }
  if (
    pointerTreeItem.treeLabelButton.targetAttr === "true" ||
    pointerTreeItem.treeDisclosureButton.targetAttr === "true"
  ) {
    hardFailures.push("folder tree inner buttons are drawing their own local edge highlight instead of the tree item owning the stroke");
  }
  if (!(pointerTileThumb.tileThumb.interactiveOpacity > 0) || pointerTileThumb.tileThumb.interactiveDataset !== "active") {
    hardFailures.push("grid thumbnails do not participate in the shared local edge highlight model");
  }
  if (
    !nearCssPercent(pointerTileThumb.tileThumb.interactiveX, 35, 2) ||
    !nearCssPercent(pointerTileThumb.tileThumb.interactiveY, 72, 2)
  ) {
    hardFailures.push("grid thumbnail pointer glow is snapped to an edge instead of following the real cursor");
  }
  if (
    !/url\(/i.test(pointerTileThumb.tileThumb.beforeBackgroundImage ?? "") ||
    !/radial-gradient/i.test(pointerTileThumb.tileThumb.beforeBackgroundImage ?? "") ||
    !/soft-light/i.test(pointerTileThumb.tileThumb.beforeBackgroundBlendMode ?? "") ||
    !/radial-gradient/i.test(
      pointerTileThumb.tileThumb.beforeMaskImage ??
        pointerTileThumb.tileThumb.beforeWebkitMaskImage ??
        ""
    )
  ) {
    hardFailures.push("interactive pointer background glow is not dithered and masked around the real cursor");
  }
  const pointerSettingsSlider = summary.evidence.settingsInterfaceStyle.pointerSlider;
  if (
    !(pointerSettingsSlider?.settingsSliderControl?.interactiveOpacity > 0) ||
    pointerSettingsSlider?.settingsSliderControl?.interactiveDataset !== "active"
  ) {
    hardFailures.push("settings range sliders do not expose a stylable local pointer-highlight owner");
  }
  if (
    !nearCssPercent(pointerSettingsSlider?.settingsSliderControl?.interactiveX, 27, 2) ||
    !nearCssPercent(pointerSettingsSlider?.settingsSliderControl?.interactiveY, 50, 2)
  ) {
    hardFailures.push("settings range slider pointer glow is not based on the real cursor position");
  }
  if (pointerSettingsSlider?.settingsSliderInput?.targetAttr === "true") {
    hardFailures.push("native range input is still selected as the local pointer-highlight target");
  }
  if (summary.evidence.previewApiResponses.length === 0) {
    hardFailures.push("preview route was not requested");
  }

  summary = { ...summary, hardFailures, fatalError: null };
  await writeFile(summaryPath, JSON.stringify(summary, null, 2));

  if (hardFailures.length) {
    throw new Error(`Desktop UI visual regression failed: ${hardFailures.join(", ")}. See ${summaryPath}`);
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
