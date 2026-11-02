import crypto from "node:crypto";
import net from "node:net";

export interface CoreSession {
  baseUrl: string;
  bindAddress: string;
  hostDbPath?: string;
  librariesDir?: string;
  sessionToken: string;
  allowedOrigin: string;
}

export async function createCoreSession(): Promise<CoreSession> {
  const isExternalCore = process.env.MANGOO_CORE_EXTERNAL === "1";
  const configuredBaseUrl = process.env.MANGOO_CORE_URL;
  const configuredBindAddress = process.env.MANGOO_CORE_ADDR;
  const sessionToken = isExternalCore
    ? requireEnv("MANGOO_SESSION_TOKEN", "MANGOO_SESSION_TOKEN is required when MANGOO_CORE_EXTERNAL=1")
    : (process.env.MANGOO_SESSION_TOKEN ?? crypto.randomBytes(32).toString("base64url"));

  if (isExternalCore) {
    return {
      baseUrl: requireEnv("MANGOO_CORE_URL", "MANGOO_CORE_URL is required when MANGOO_CORE_EXTERNAL=1"),
      bindAddress: configuredBindAddress ?? "external",
      hostDbPath: process.env.MANGOO_HOST_DB_PATH,
      librariesDir: process.env.MANGOO_LIBRARIES_DIR,
      sessionToken,
      allowedOrigin:
        process.env.MANGOO_ALLOWED_ORIGIN ?? process.env.MANGOO_WEB_URL ?? "http://127.0.0.1:5173"
    };
  }

  const urlEndpoint = configuredBaseUrl ? endpointFromCoreUrl(configuredBaseUrl) : undefined;
  const bindAddress = configuredBindAddress
    ? normalizeBindAddress(configuredBindAddress, "MANGOO_CORE_ADDR")
    : (urlEndpoint?.bindAddress ?? `127.0.0.1:${await chooseLoopbackPort()}`);

  if (configuredBindAddress && urlEndpoint && endpointKey(bindAddress) !== endpointKey(urlEndpoint.bindAddress)) {
    throw new Error("MANGOO_CORE_ADDR and MANGOO_CORE_URL must point to the same host/port");
  }

  const baseUrl = configuredBaseUrl ?? `http://${bindAddress}/api`;
  const packagedRendererOrigin = process.env.MANGOO_SERVE_WEB === "1"
    ? `http://${bindAddress}`
    : undefined;

  return {
    baseUrl,
    bindAddress,
    hostDbPath: process.env.MANGOO_HOST_DB_PATH,
    librariesDir: process.env.MANGOO_LIBRARIES_DIR,
    sessionToken,
    allowedOrigin:
      process.env.MANGOO_ALLOWED_ORIGIN ??
      process.env.MANGOO_WEB_URL ??
      packagedRendererOrigin ??
      "http://127.0.0.1:5173"
  };
}

async function chooseLoopbackPort(): Promise<number> {
  // OS ephemeral ranges can include Chromium-blocked ports (e.g. 2049).
  // Both packaged pages and renderer API requests need a browser-usable port.
  let lastError: unknown;
  for (let attempt = 0; attempt < 128; attempt += 1) {
    try {
      return await probeLoopbackPort(crypto.randomInt(49152, 65536));
    } catch (error) {
      if (!(error instanceof Error) || !("code" in error) ||
          (error.code !== "EACCES" && error.code !== "EADDRINUSE")) throw error;
      lastError = error;
    }
  }
  throw new Error("Could not allocate a browser-usable loopback Core port after 128 attempts", { cause: lastError });
}

async function probeLoopbackPort(candidate: number): Promise<number> {
  return new Promise((resolve, reject) => {
    const server = net.createServer();
    server.once("error", reject);
    server.listen(candidate, "127.0.0.1", () => {
      const address = server.address();
      if (!address || typeof address === "string") {
        server.close();
        reject(new Error("Could not allocate a loopback Core port"));
        return;
      }

      const { port } = address;
      server.close((error) => {
        if (error) {
          reject(error);
          return;
        }
        resolve(port);
      });
    });
  });
}

function endpointFromCoreUrl(value: string): { bindAddress: string } {
  const parsed = new URL(value);
  const port = parsed.port || defaultPortForProtocol(parsed.protocol);
  return {
    bindAddress: formatBindAddress(normalizeLoopbackHost(parsed.hostname), port, "MANGOO_CORE_URL")
  };
}

function defaultPortForProtocol(protocol: string): string {
  return protocol === "https:" ? "443" : "80";
}

function normalizeBindAddress(value: string, source: string): string {
  const parsed = parseHostPort(value, source);
  return formatBindAddress(normalizeLoopbackHost(parsed.host), parsed.port, source);
}

function parseHostPort(value: string, source: string): { host: string; port: string } {
  if (value.startsWith("[")) {
    const endBracket = value.indexOf("]");
    if (endBracket === -1 || value[endBracket + 1] !== ":") {
      throw new Error(`${source} must be a host:port bind address`);
    }
    return {
      host: value.slice(1, endBracket),
      port: value.slice(endBracket + 2)
    };
  }

  const separator = value.lastIndexOf(":");
  if (separator === -1 || value.indexOf(":") !== separator) {
    throw new Error(`${source} must be a host:port bind address`);
  }
  return {
    host: value.slice(0, separator),
    port: value.slice(separator + 1)
  };
}

function normalizeLoopbackHost(host: string): string {
  const unbracketedHost = host.startsWith("[") && host.endsWith("]") ? host.slice(1, -1) : host;
  return unbracketedHost.toLowerCase() === "localhost" ? "127.0.0.1" : unbracketedHost;
}

function formatBindAddress(host: string, port: string, source: string): string {
  if (!/^\d+$/.test(port)) {
    throw new Error(`${source} must include a numeric port`);
  }
  if (!isLoopbackOrIpLiteral(host)) {
    throw new Error(`Unsupported Core host "${host}" in ${source}; use localhost or an IP literal`);
  }
  return net.isIP(host) === 6 ? `[${host}]:${port}` : `${host}:${port}`;
}

function endpointKey(bindAddress: string): string {
  const parsed = parseHostPort(bindAddress, "Core endpoint");
  return `${normalizeLoopbackHost(parsed.host).toLowerCase()}:${parsed.port}`;
}

function isLoopbackOrIpLiteral(host: string): boolean {
  const ipVersion = net.isIP(host);
  return ipVersion !== 0;
}

function requireEnv(name: string, message: string): string {
  const value = process.env[name];
  if (!value) {
    throw new Error(message);
  }
  return value;
}
