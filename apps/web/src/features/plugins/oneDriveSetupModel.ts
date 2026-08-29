import type { OneDriveConfiguration } from "@megle/core-client";

export type OneDriveSetupStep = 1 | 2 | 3 | 4 | 5;

export const ONEDRIVE_CLIENT_ID_PATTERN =
  /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

export function isValidOneDriveClientId(value: string): boolean {
  const normalized = value.trim();
  return ONEDRIVE_CLIENT_ID_PATTERN.test(normalized)
    && normalized.toLowerCase() !== "00000000-0000-0000-0000-000000000000";
}

export function deriveOneDriveSetupStep(
  configuration: OneDriveConfiguration | null,
  authenticated: boolean,
): OneDriveSetupStep {
  if (authenticated) return 5;
  if (configuration?.configured) return 4;
  return 1;
}

export function oneDriveSetupRecoveryStep(message: string | null): OneDriveSetupStep | null {
  if (!message) return null;
  const normalized = message.toLowerCase();
  if (/client.?id|application.*not found|aadsts700016|identifier/.test(normalized)) return 3;
  if (/redirect|reply url|public client|pkce|permission|consent|tenant|admin|audience/.test(normalized)) return 2;
  return null;
}
