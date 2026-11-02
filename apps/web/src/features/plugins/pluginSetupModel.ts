import type { PluginConfiguration, PluginConfigurationField } from "@mangoo/core-client";

export type PluginSetupStep = 1 | 2 | 3 | 4;

export function visibleConfigurationFields(configuration: PluginConfiguration | null): PluginConfigurationField[] {
  return configuration?.fields.filter((field) => !field.hidden && !field.debugOnly) ?? [];
}

export function configurationFieldError(field: PluginConfigurationField, value: string): string | null {
  const normalized = value.trim();
  if (!normalized) return field.required ? `请输入${field.label}。` : null;
  if (normalized.length > 2048 || /[\u0000-\u001f\u007f]/.test(normalized)) return "内容过长或包含无效字符。";
  switch (field.format) {
    case "uuid": return /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(normalized)
      && normalized !== "00000000-0000-0000-0000-000000000000" ? null : "请输入有效的 UUID。";
    case "choice": return field.choices.includes(normalized) ? null : "请选择声明的选项。";
    case "https-url":
    case "loopback-url": {
      try {
        const url = new URL(normalized);
        const loopback = ["localhost", "127.0.0.1", "[::1]"].includes(url.hostname);
        if (url.username || url.password || url.hash || /[\s\\#]/.test(normalized)) return "请输入有效的地址。";
        return (field.format === "https-url" ? url.protocol === "https:" : loopback && ["http:", "https:"].includes(url.protocol)) ? null : "地址不符合插件要求。";
      } catch { return "请输入有效的地址。"; }
    }
    default: return null;
  }
}

export function derivePluginSetupStep(configuration: PluginConfiguration | null, authenticated: boolean): PluginSetupStep {
  if (authenticated) return 4;
  return configuration?.configured ? 3 : 1;
}
