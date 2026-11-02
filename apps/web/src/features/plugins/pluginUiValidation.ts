/** Runtime decoders for the isolated document boundary. Core remains authoritative for domain invariants. */
export type Rule = (value: unknown) => boolean;
export const record = (value: unknown): value is Record<string, unknown> => value !== null && typeof value === "object" && !Array.isArray(value);
export const text: Rule = (value) => typeof value === "string" && value.length > 0 && value.length <= 32768;
export const integer: Rule = (value) => typeof value === "number" && Number.isSafeInteger(value) && value >= 0;
export const boolean: Rule = (value) => typeof value === "boolean";
export const optional = (rule: Rule): Rule => (value) => value === undefined || rule(value);
export const nullable = (rule: Rule): Rule => (value) => value === null || rule(value);
export const oneOf = (...values: string[]): Rule => (value) => typeof value === "string" && values.includes(value);
export const array = (rule: Rule): Rule => (value) => Array.isArray(value) && value.length <= 1000 && value.every(rule);
export const object = (fields: Record<string, Rule>): Rule => (value) => record(value) && Object.keys(value).every((key) => Object.hasOwn(fields, key)) && Object.entries(fields).every(([key, rule]) => rule(value[key]));
export const tuple = (rules: Rule[], values: unknown[]): boolean => values.length <= rules.length && rules.every((rule, index) => rule(values[index]));
export const options = optional(object({ requestPriority: optional(oneOf("interactive", "navigation", "metadata", "background")) }));
export const stringPage = optional(object({ cursor: optional(text), limit: optional(integer) }));
export const numericPage = optional(object({ cursor: optional(integer), limit: optional(integer) }));

export function isBoundedJson(value: unknown, maxBytes = 1024 * 1024): boolean {
  let nodes = 0;
  const walk = (item: unknown, depth: number): boolean => {
    if (++nodes > 20000 || depth > 20) return false;
    if (item === null || typeof item === "boolean") return true;
    if (typeof item === "number") return Number.isFinite(item);
    if (typeof item === "string") return item.length <= maxBytes;
    if (Array.isArray(item)) return item.every((child) => walk(child, depth + 1));
    if (!record(item)) return false;
    return Object.entries(item).every(([key, child]) => key !== "__proto__" && key !== "constructor" && key !== "prototype" && walk(child, depth + 1));
  };
  try { return walk(value, 0) && new TextEncoder().encode(JSON.stringify(value)).length <= maxBytes; } catch { return false; }
}
