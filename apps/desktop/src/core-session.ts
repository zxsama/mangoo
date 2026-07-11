import crypto from "node:crypto";
import net from "node:net";
import path from "node:path";

export interface CoreSession {
  baseUrl: string;
  bindAddress: string;
  dbPath: string;
  sessionToken: string;
  allowedOrigin: string;
}

export async function createCoreSession(): Promise<CoreSession> {
  const isExternalCore = process.env.MEGLE_CORE_EXTERNAL === "1";
  const configuredBaseUrl = process.env.MEGLE_CORE_URL;
  const configuredBindAddress = process.env.MEGLE_CORE_ADDR;
  const sessionToken = isExternalCore
    ? requireEnv("MEGLE_SESSION_TOKEN", "MEGLE_SESSION_TOKEN is required when MEGLE_CORE_EXTERNAL=1")
    : (process.env.MEGLE_SESSION_TOKEN ?? crypto.randomBytes(32).toString("base64url"));

  if (isExternalCore) {
    return {
      baseUrl: requireEnv("MEGLE_CORE_URL", "MEGLE_CORE_URL is required when MEGLE_CORE_EXTERNAL=1"),
      bindAddress: configuredBindAddress ?? "external",
      dbPath: process.env.MEGLE_DB_PATH ?? path.join(process.cwd(), "megle-dev.sqlite"),
      sessionToken,
      allowedOrigin:
        process.env.MEGLE_ALLOWED_ORIGIN ?? process.env.MEGLE_WEB_URL ?? "http://127.0.0.1:5173"
    };
  }

  const urlEndpoint = configuredBaseUrl ? endpointFromCoreUrl(configuredBaseUrl) : undefined;
  const bindAddress = configuredBindAddress
    ? normalizeBindAddress(configuredBindAddress, "MEGLE_CORE_ADDR")
    : (urlEndpoint?.bindAddress ?? `127.0.0.1:${await chooseLoopbackPort()}`);

  if (configuredBindAddress && urlEndpoint && endpointKey(bindAddress) !== endpointKey(urlEndpoint.bindAddress)) {
    throw new Error("MEGLE_CORE_ADDR and MEGLE_CORE_URL must point to the same host/port");
  }

  const baseUrl = configuredBaseUrl ?? `http://${bindAddress}/api`;

  return {
    baseUrl,
    bindAddress,
    dbPath: process.env.MEGLE_DB_PATH ?? path.join(process.cwd(), "megle-dev.sqlite"),
    sessionToken,
    allowedOrigin:
      process.env.MEGLE_ALLOWED_ORIGIN ?? process.env.MEGLE_WEB_URL ?? "http://127.0.0.1:5173"
  };
}

async function chooseLoopbackPort(): Promise<number> {
  return new Promise((resolve, reject) => {
    const server = net.createServer();
    server.once("error", reject);
    server.listen(0, "127.0.0.1", () => {
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
    bindAddress: formatBindAddress(normalizeLoopbackHost(parsed.hostname), port, "MEGLE_CORE_URL")
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

// Build a session for a different database file that reuses the same bind
// address + session token, so a Core restart does not require the renderer to
// re-read its coreUrl/token (spec Decision 6).
export function sessionForDbPath(base: CoreSession, dbPath: string): CoreSession {
  return { ...base, dbPath };
}

function requireEnv(name: string, message: string): string {
  const value = process.env[name];
  if (!value) {
    throw new Error(message);
  }
  return value;
}
