import { readFileSync } from "node:fs";
import path from "node:path";
import process from "node:process";
import { parse } from "yaml";

const root = path.resolve(import.meta.dirname, "..", "..", "..");

function read(relativePath) {
  // Keep contract regex checks stable on Windows checkouts with CRLF endings.
  return readFileSync(path.join(root, relativePath), "utf8").replace(/\r\n/g, "\n");
}

function fail(message) {
  console.error(`FAIL: ${message}`);
  process.exitCode = 1;
}

function dereference(value) {
  if (!value?.$ref) return value;
  const parts = value.$ref.replace(/^#\//, "").split("/");
  let current = contract;
  for (const part of parts) {
    current = current?.[part];
  }
  return current;
}

function schemaAllowsNull(schema) {
  const value = dereference(schema);
  return (
    value?.nullable === true ||
    value?.type === "null" ||
    (Array.isArray(value?.type) && value.type.includes("null")) ||
    value?.anyOf?.some(schemaAllowsNull) ||
    value?.oneOf?.some(schemaAllowsNull)
  );
}

function schemaTsType(schema) {
  if (schema?.$ref) return schema.$ref.split("/").at(-1);
  const value = dereference(schema);
  if (!value) return "unknown";
  if (value.anyOf || value.oneOf) {
    const variants = [...(value.anyOf ?? []), ...(value.oneOf ?? [])]
      .filter((variant) => dereference(variant)?.type !== "null")
      .map(schemaTsType);
    return [...new Set(variants)].join(" | ") || "null";
  }
  if (value.$ref) return value.$ref.split("/").at(-1);
  if (value.type === "array") return `${schemaTsType(value.items)}[]`;
  if (value.enum) return value.enum.map((item) => JSON.stringify(item)).join(" | ");
  if (value.type === "integer" || value.type === "number") return "number";
  if (value.type === "string") return "string";
  if (value.type === "boolean") return "boolean";
  return "unknown";
}

function expectedPropertyLine(name, schema, required) {
  const optional = required ? "" : "?";
  const nullable = schemaAllowsNull(schema) ? " | null" : "";
  return `${name}${optional}: ${schemaTsType(schema)}${nullable};`;
}

function interfaceBody(name) {
  const pattern = new RegExp(
    `export\\s+interface\\s+${name}(?:<[^>]+>)?(?:\\s+extends[^\\{]+)?\\s*{([\\s\\S]*?)\\n}`
  );
  const match = generated.match(pattern);
  return match?.[1] ?? "";
}

function requireLine(body, line, context) {
  if (!body.includes(line)) {
    fail(`${context} must include '${line}'`);
  }
}

function requireNotLine(body, line, context) {
  if (body.includes(line)) {
    fail(`${context} must not include '${line}'`);
  }
}

function schema(name) {
  const value = schemas[name];
  if (!value) {
    fail(`OpenAPI contract missing schema ${name}`);
  }
  return value;
}

function assertInterfaceMatchesSchema(name) {
  const openApiSchema = schema(name);
  const body = interfaceBody(name);
  if (!body) {
    fail(`generated-contract.ts missing interface ${name}`);
    return;
  }

  const required = new Set(openApiSchema.required ?? []);
  for (const [propertyName, propertySchema] of Object.entries(openApiSchema.properties ?? {})) {
    const expected = expectedPropertyLine(propertyName, propertySchema, required.has(propertyName));
    requireLine(body, expected, `generated-contract.ts ${name}`);

    if (!required.has(propertyName)) {
      requireNotLine(
        body,
        expectedPropertyLine(propertyName, propertySchema, true),
        `generated-contract.ts ${name}`
      );
    }
  }
}

function generatedTypeBody(name) {
  const match = generated.match(new RegExp(`export\\s+type\\s+${name}\\s*=([\\s\\S]*?);`));
  return match?.[1]?.replace(/\s+/g, " ").replace(/^\s*\|\s*/, "").trim() ?? "";
}

function assertEnumMatchesGeneratedType(name) {
  const expected = (schema(name).enum ?? []).map((item) => JSON.stringify(item)).join(" | ");
  const actual = generatedTypeBody(name);
  if (actual !== expected) {
    fail(`generated-contract.ts ${name} must equal OpenAPI enum ${expected}`);
  }
}

function parameterName(parameter) {
  return dereference(parameter)?.name;
}

function operation(operationId) {
  for (const [route, pathItem] of Object.entries(contract.paths ?? {})) {
    for (const [method, candidate] of Object.entries(pathItem ?? {})) {
      if (candidate?.operationId === operationId) {
        return { route, method, operation: candidate };
      }
    }
  }
  fail(`OpenAPI contract missing operation ${operationId}`);
  return null;
}

function assertOperationParameters(operationId, names) {
  const found = operation(operationId);
  if (!found) return;
  const actual = (found.operation.parameters ?? []).map(parameterName);
  for (const name of names) {
    if (!actual.includes(name)) {
      fail(`OpenAPI operation ${operationId} missing parameter ${name}`);
    }
  }
}

function assertExactOperationParameters(operationId, names) {
  const found = operation(operationId);
  if (!found) return;
  const actual = (found.operation.parameters ?? []).map(parameterName);
  if (actual.length !== names.length || names.some((name) => !actual.includes(name))) {
    fail(`OpenAPI operation ${operationId} parameters must be exactly ${names.join(", ")}`);
  }
}

function assertOperationRoute(operationId, method, route) {
  const found = operation(operationId);
  if (!found) return;
  if (found.method !== method || found.route !== route) {
    fail(`OpenAPI operation ${operationId} must be ${method.toUpperCase()} ${route}`);
  }
}

function operationParameter(operationId, name) {
  const found = operation(operationId);
  return (found?.operation.parameters ?? [])
    .map(dereference)
    .find((parameter) => parameter?.name === name);
}

function assertIntegerParameterBounds(operationId, name, minimum, maximum) {
  const parameter = operationParameter(operationId, name);
  if (!parameter) {
    fail(`OpenAPI operation ${operationId} missing parameter ${name}`);
    return;
  }
  const parameterSchema = dereference(parameter.schema);
  if (parameterSchema?.type !== "integer") {
    fail(`OpenAPI parameter ${name} for ${operationId} must be an integer`);
  }
  if (parameterSchema?.minimum !== minimum) {
    fail(`OpenAPI parameter ${name} for ${operationId} must have minimum ${minimum}`);
  }
  if (parameterSchema?.maximum !== maximum) {
    fail(`OpenAPI parameter ${name} for ${operationId} must have maximum ${maximum}`);
  }
}

function assertIntegerSchemaPropertyBounds(schemaName, propertyName, minimum, maximum) {
  const propertySchema = dereference(schema(schemaName)?.properties?.[propertyName]);
  if (propertySchema?.type !== "integer") {
    fail(`OpenAPI ${schemaName}.${propertyName} must be an integer`);
  }
  if (propertySchema?.minimum !== minimum) {
    fail(`OpenAPI ${schemaName}.${propertyName} must have minimum ${minimum}`);
  }
  if (propertySchema?.maximum !== maximum) {
    fail(`OpenAPI ${schemaName}.${propertyName} must have maximum ${maximum}`);
  }
}

function assertParamLine(interfaceName, parameterName, line) {
  const parameter = operationParameter(
    interfaceName === "ListMediaParams" ? "listMedia" : "listFolderChildren",
    parameterName
  );
  if (!parameter) {
    fail(`OpenAPI parameter ${parameterName} missing for ${interfaceName}`);
    return;
  }
  requireLine(interfaceBody(interfaceName), line, `generated-contract.ts ${interfaceName}`);
}

const contract = parse(read("contracts/core-api/openapi.yaml"));
const generated = read("packages/core-client/src/generated-contract.ts");
const client = read("packages/core-client/src/client.ts");

const schemas = contract.components?.schemas ?? {};

if (schemas.ListNotificationOutboxParams) {
  fail("OpenAPI must define notification outbox query parameters on the endpoint, not a duplicate component schema");
}

for (const name of [
  "AcceptedRootResponse",
  "ScanTaskRequest",
  "InteractiveFolderScanTaskRequest",
  "ThumbnailPriorityScopeSyncRequest",
  "ThumbnailPriority",
  "ScanSummary",
  "RootRecord",
  "RootListResponse",
  "FolderRecord",
  "FolderListResponse",
  "MediaRecord",
  "MediaPositionResponse",
  "MediaListResponse",
  "ThumbnailFailureRecord",
  "ThumbnailFailureListResponse",
  "ThumbnailCacheSnapshotResponse",
  "ThumbnailAsset",
  "ThumbnailResponse",
  "TaskKind",
  "TaskStatus",
  "TaskRecord",
  "TaskListResponse",
  "NotificationOrigin",
  "NotificationPresentation",
  "NotificationOperationStatus",
  "NotificationOperationSnapshot",
  "NotificationOutboxEvent",
  "NotificationOutboxResponse",
  "NotificationOutboxAckRequest",
  "NotificationOutboxAckResponse",
  "TagRecord",
  "TagListResponse",
  "CreateTagRequest",
  "DeleteTagResponse",
  "UserMetadataRecord",
  "UserMetadataUpdate",
  "FileTagsResponse",
  "AddFileTagRequest",
  "SetFileTagsRequest",
  "FileOperationRecord",
  "FileOperationsResponse",
  "FileOperationListResponse",
  "RenameRequest",
  "MoveRequest",
  "DeleteRequest",
  "HostPluginProjection",
  "HostPluginListResponse",
  "PluginDiscoveryError",
  "PluginDiscoveryResponse",
  "PluginCommandResponse",
  "PluginGrantRequest",
  "PluginVersionRequest",
  "PluginAuthBeginResponse",
  "PluginAuthCompleteRequest",
  "PluginHealthResponse",
  "PluginDiagnosticsResponse"
]) {
  schema(name);
}

assertInterfaceMatchesSchema("ScanSummary");
assertInterfaceMatchesSchema("LibrarySummary");
assertInterfaceMatchesSchema("LibraryListResponse");
assertInterfaceMatchesSchema("ActiveLibraryResponse");
assertInterfaceMatchesSchema("LibraryPreflightRequest");
assertInterfaceMatchesSchema("LibraryPreflightResponse");
assertInterfaceMatchesSchema("CreateLibraryRequest");
assertInterfaceMatchesSchema("UpdateLibraryRequest");
assertInterfaceMatchesSchema("CreateLibraryResponse");
assertInterfaceMatchesSchema("LibraryErrorResponse");
assertInterfaceMatchesSchema("AcceptedRootResponse");
assertInterfaceMatchesSchema("ScanTaskRequest");
assertInterfaceMatchesSchema("InteractiveFolderScanTaskRequest");
assertInterfaceMatchesSchema("ThumbnailPriorityScopeSyncRequest");
assertInterfaceMatchesSchema("RootRecord");
assertInterfaceMatchesSchema("FolderRecord");
assertInterfaceMatchesSchema("MediaRecord");
assertInterfaceMatchesSchema("MediaPositionResponse");
assertInterfaceMatchesSchema("ThumbnailFailureRecord");
assertInterfaceMatchesSchema("ThumbnailAsset");
assertInterfaceMatchesSchema("ThumbnailResponse");
assertInterfaceMatchesSchema("TaskRecord");
assertInterfaceMatchesSchema("NotificationOperationSnapshot");
assertInterfaceMatchesSchema("NotificationOutboxEvent");
assertInterfaceMatchesSchema("NotificationOutboxResponse");
assertInterfaceMatchesSchema("NotificationOutboxAckRequest");
assertInterfaceMatchesSchema("NotificationOutboxAckResponse");
assertInterfaceMatchesSchema("TagRecord");
assertInterfaceMatchesSchema("CreateTagRequest");
assertInterfaceMatchesSchema("DeleteTagResponse");
assertInterfaceMatchesSchema("UserMetadataRecord");
assertInterfaceMatchesSchema("FileTagsResponse");
assertInterfaceMatchesSchema("AddFileTagRequest");
assertInterfaceMatchesSchema("SetFileTagsRequest");
assertInterfaceMatchesSchema("FileOperationRecord");
assertInterfaceMatchesSchema("FileOperationsResponse");
assertInterfaceMatchesSchema("FileOperationListResponse");
assertInterfaceMatchesSchema("RenameRequest");
assertInterfaceMatchesSchema("MoveRequest");
assertInterfaceMatchesSchema("DeleteRequest");
assertInterfaceMatchesSchema("HostPluginProjection");
assertInterfaceMatchesSchema("PluginDiscoveryError");
assertInterfaceMatchesSchema("PluginDiscoveryResponse");
assertInterfaceMatchesSchema("PluginCommandResponse");

const fileOperationSchema = schema("FileOperationRecord");
const operationGroupSchema = fileOperationSchema.properties?.operationGroupId;
if (!operationGroupSchema) {
  fail("OpenAPI FileOperationRecord must expose operationGroupId");
} else {
  if ((fileOperationSchema.required ?? []).includes("operationGroupId")) {
    fail("OpenAPI FileOperationRecord.operationGroupId must remain optional");
  }
  if (!schemaAllowsNull(operationGroupSchema)) {
    fail("OpenAPI FileOperationRecord.operationGroupId must remain nullable");
  }
}

for (const name of ["NotificationOrigin", "NotificationPresentation", "NotificationOperationStatus"]) {
  assertEnumMatchesGeneratedType(name);
}

const notificationSnapshotBody = interfaceBody("NotificationOperationSnapshot");
requireLine(
  notificationSnapshotBody,
  'category: "task" | "file_operation" | "error" | "thumbnail_cache";',
  "generated-contract.ts NotificationOperationSnapshot"
);

const pageBody = interfaceBody("Page");
for (const line of ["items: T[];", "nextCursor: string | null;", "totalCount?: number;"]) {
  requireLine(pageBody, line, "generated-contract.ts Page");
}

const listMediaBody = interfaceBody("ListMediaParams");
for (const [name, line] of [
  ["rootId", "rootId?: number;"],
  ["folderId", "folderId?: number;"],
  ["limit", "limit?: number;"],
  ["offset", "offset?: number;"],
  ["cursor", "cursor?: string;"],
  ["sort", `sort?: ${operationParameter("listMedia", "sort")?.schema?.enum.map((item) => JSON.stringify(item)).join(" | ")};`],
  ["kind", `kind?: ${operationParameter("listMedia", "kind")?.schema?.enum.map((item) => JSON.stringify(item)).join(" | ")};`]
]) {
  assertParamLine("ListMediaParams", name, line);
}

const listFolderChildrenBody = interfaceBody("ListFolderChildrenParams");
if (!listFolderChildrenBody) {
  fail("generated-contract.ts missing ListFolderChildrenParams");
}
for (const [name, line] of [
  ["limit", "limit?: number;"],
  ["cursor", "cursor?: string;"],
  ["includeDescendants", "includeDescendants?: boolean;"]
]) {
  assertParamLine("ListFolderChildrenParams", name, line);
}

assertOperationParameters("listFolderChildren", ["folderId", "limit", "cursor", "includeDescendants"]);
assertOperationParameters("listFolderAncestors", ["folderId"]);
assertOperationParameters("listMedia", ["rootId", "folderId", "limit", "offset", "cursor", "sort", "kind"]);
assertOperationParameters("getMedia", ["fileId"]);
assertOperationParameters("getMediaPosition", ["fileId", "rootId", "folderId", "includeDescendants", "sort"]);
assertOperationRoute("getMediaPosition", "get", "/media/{fileId}/position");
assertOperationRoute("listLibraries", "get", "/libraries");
assertOperationRoute("createLibrary", "post", "/libraries");
assertOperationRoute("getActiveLibrary", "get", "/libraries/active");
assertOperationRoute("preflightLibrary", "post", "/libraries/preflight");
assertOperationRoute("activateLibrary", "post", "/libraries/{libraryUuid}/activate");
assertOperationRoute("removeLocalLibrary", "delete", "/libraries/{libraryUuid}/local-copy");
assertOperationParameters("listThumbnailCacheFailures", ["limit"]);
assertOperationParameters("getThumbnail", ["fileId", "target", "priority"]);
assertOperationParameters("getThumbnailBlob", ["fileId", "target"]);
assertOperationParameters("getPreview", ["fileId", "viewportLongPx"]);
// The declared bounds must stay pinned: the route clamps to the maximum and
// 400s a non-integer, so a contract that drifted from these numbers would be
// promising behaviour the server does not implement.
assertIntegerParameterBounds("getPreview", "viewportLongPx", 1, 16384);
operation("syncThumbnailPriorityScope");
assertOperationParameters("searchMedia", ["rootId", "folderId", "limit", "offset", "cursor", "sort", "kind"]);
assertOperationRoute("listNotificationOutbox", "get", "/notifications/outbox");
assertOperationRoute("ackNotificationOutbox", "post", "/notifications/outbox/ack");
assertExactOperationParameters("listNotificationOutbox", ["afterSequence", "limit", "waitMs"]);
assertIntegerParameterBounds("listNotificationOutbox", "afterSequence", 0, 9007199254740991);
assertIntegerParameterBounds("listNotificationOutbox", "limit", 1, 200);
assertIntegerParameterBounds("listNotificationOutbox", "waitMs", 0, 30000);
assertIntegerSchemaPropertyBounds("NotificationOutboxEvent", "sequence", 0, 9007199254740991);
assertIntegerSchemaPropertyBounds(
  "NotificationOutboxResponse",
  "maxSequence",
  0,
  9007199254740991
);
assertIntegerSchemaPropertyBounds(
  "NotificationOutboxAckRequest",
  "maxSequence",
  0,
  9007199254740991
);

const listNotificationOutboxBody = interfaceBody("ListNotificationOutboxParams");
for (const line of [
  "afterSequence?: number;",
  "limit?: number;",
  "waitMs?: number;"
]) {
  requireLine(listNotificationOutboxBody, line, "generated-contract.ts ListNotificationOutboxParams");
}

for (const method of [
  "listRoots",
  "addRoot",
  "removeRoot",
  "enqueueScan",
  "enqueueInteractiveFolderScan",
  "syncThumbnailPriorityScope",
  "listFolderChildren",
  "listFolderAncestors",
  "listMedia",
  "getMedia",
  "getMediaPosition",
  "getThumbnail",
  "getThumbnailBlob",
  "listThumbnailCacheFailures",
  "getPreviewBlob",
  "getPreviewResource",
  "listTasks",
  "cancelTask",
  "retryTask",
  "listTags",
  "createTag",
  "deleteTag",
  "getUserMetadata",
  "updateUserMetadata",
  "setFileTags",
  "addFileTag",
  "removeFileTag",
  "searchMedia",
  "renameFileOp",
  "moveFileOps",
  "deleteFileOps",
  "listFileOperations",
  "listNotificationOutbox",
  "ackNotificationOutbox",
  "listPlugins",
  "getPlugin",
  "discoverPlugins",
  "enablePlugin",
  "disablePlugin",
  "unregisterPlugin",
  "updatePluginGrants",
  "retryPlugin",
  "selectPluginVersion",
  "rollbackPluginVersion",
  "beginPluginAuth",
  "completePluginAuth",
  "signOutPlugin",
  "getPluginHealth",
  "getPluginDiagnostics"
]) {
  if (!client.includes(`${method}:`)) {
    fail(`client.ts missing operation ${method}`);
  }
}

if (
  !/listNotificationOutbox:\s*\(\s*params:\s*ListNotificationOutboxParams,\s*options:\s*CoreRequestOptions\s*=\s*\{\}\s*\)\s*=>[\s\S]*?request<NotificationOutboxResponse>\(\s*`\/notifications\/outbox\$\{notificationOutboxQuery\(params\)\}`,\s*\{[\s\S]*?requestPriority:\s*options\.requestPriority\s*\?\?\s*"metadata",[\s\S]*?signal:\s*options\.signal[\s\S]*?\}\s*\)/.test(
    client
  )
) {
  fail("client.ts listNotificationOutbox must serialize the typed query and forward metadata priority overrides and abort signals");
}

if (
  !/ackNotificationOutbox:\s*\(body:\s*NotificationOutboxAckRequest\)\s*=>\s*request<NotificationOutboxAckResponse>\("\/notifications\/outbox\/ack",\s*\{[\s\S]*?method:\s*"POST",[\s\S]*?requestPriority:\s*"metadata",[\s\S]*?body:\s*JSON\.stringify\(body\)[\s\S]*?\}\s*\)/.test(
    client
  )
) {
  fail("client.ts ackNotificationOutbox must POST the typed acknowledgement at metadata priority");
}

if (
  !/function notificationOutboxQuery\(params:\s*ListNotificationOutboxParams\):\s*string[\s\S]*?typeof params\.afterSequence !== "undefined"[\s\S]*?typeof params\.limit !== "undefined"[\s\S]*?typeof params\.waitMs !== "undefined"/.test(
    client
  )
) {
  fail("client.ts notificationOutboxQuery must serialize only defined query values");
}

const thumbnailBody = interfaceBody("ThumbnailResponse");
for (const line of [
  'target: "grid_320";',
  'state: "pending" | "queued" | "ready" | "failed" | "skipped_small";',
  'shortSidePx: number;',
  'outputFormat: "image/webp";',
  "width: number | null;",
  "height: number | null;",
  "byteSize: number | null;",
  'servedBy: "db_blob" | null;',
  "asset: ThumbnailAsset | null;",
  "error: string | null;"
]) {
  requireLine(thumbnailBody, line, "generated-contract.ts ThumbnailResponse");
}

const mediaBody = interfaceBody("MediaRecord");
for (const line of [
  "previewPlaceholder?: number[] | null;",
  "previewPlaceholderFormat?: string | null;"
]) {
  requireLine(mediaBody, line, "generated-contract.ts MediaRecord");
}

const thumbnailAssetBody = interfaceBody("ThumbnailAsset");
for (const line of [
  "width: number;",
  "height: number;",
  "byteSize: number;"
]) {
  requireLine(thumbnailAssetBody, line, "generated-contract.ts ThumbnailAsset");
}
requireNotLine(thumbnailAssetBody, "cacheKey:", "generated-contract.ts ThumbnailAsset");

const taskBody = interfaceBody("TaskRecord");
for (const line of [
  "kind: TaskKind;",
  "status: TaskStatus;",
  "folderId: number | null;",
  "itemsSeen: number;",
  "itemsTotal: number | null;",
  "foldersSeen: number;",
  "mediaFilesSeen: number;",
  "skippedFiles: number;"
]) {
  requireLine(taskBody, line, "generated-contract.ts TaskRecord");
}

for (const line of [
  'export type TaskKind = "root_scan" | "interactive_folder_scan" | "thumbnail" | "root_purge";',
  'export type TaskStatus = "pending" | "running" | "succeeded" | "failed" | "cancelled";'
]) {
  if (!generated.includes(line)) {
    fail(`generated-contract.ts missing '${line}'`);
  }
}

for (const line of [
  'export type FileOperationKind =',
  '  | "rename"',
  '  | "move"',
  '  | "delete_recycle"',
  '  | "delete_permanent";',
  'export type FileOperationStatus = "succeeded" | "failed";'
]) {
  if (!generated.includes(line)) {
    fail(`generated-contract.ts missing '${line}'`);
  }
}

if (
  !/listTasks:\s*\(\s*options:\s*CoreRequestOptions\s*=\s*\{\}\s*\)\s*=>\s*request<Page<TaskRecord>>\("\/tasks",\s*\{[\s\S]*?requestPriority:\s*options\.requestPriority\s*\?\?\s*"navigation",[\s\S]*?signal:\s*options\.signal\s*\}\)/.test(
    client
  )
) {
  fail("client.ts listTasks must request typed task pages and forward abort signals");
}

if (!/cancelTask:\s*\(taskId:\s*number\)\s*=>\s*request<AcceptedRootResponse>\(`\/tasks\/\$\{taskId\}\/cancel`,\s*\{[\s\S]*?method:\s*"POST",[\s\S]*?requestPriority:\s*"interactive"[\s\S]*?\}\)/.test(client)) {
  fail("client.ts cancelTask must call POST /tasks/{taskId}/cancel");
}

if (!/retryTask:\s*\(taskId:\s*number\)\s*=>\s*request<AcceptedRootResponse>\(`\/tasks\/\$\{taskId\}\/retry`,\s*\{[\s\S]*?method:\s*"POST",[\s\S]*?requestPriority:\s*"interactive"[\s\S]*?\}\)/.test(client)) {
  fail("client.ts retryTask must call POST /tasks/{taskId}/retry");
}

if (
  !/listFolderChildren:\s*\(\s*folderId:\s*number,\s*params:\s*ListFolderChildrenParams\s*=\s*\{\},\s*options:\s*CoreRequestOptions\s*=\s*\{\}\s*\)\s*=>[\s\S]*?request<Page<FolderRecord>>\(`\/folders\/\$\{folderId\}\/children\$\{query\(params\)\}`,\s*\{[\s\S]*?requestPriority:\s*options\.requestPriority\s*\?\?\s*"navigation",[\s\S]*?signal:\s*options\.signal\s*\}\)/.test(
    client
  )
) {
  fail("client.ts listFolderChildren must accept typed limit/cursor params, serialize them, and forward abort signals");
}

if (
  !/listFolderAncestors:\s*\(\s*folderId:\s*number,\s*options:\s*CoreRequestOptions\s*=\s*\{\}\s*\)\s*=>[\s\S]*?request<Page<FolderRecord>>\(`\/folders\/\$\{folderId\}\/ancestors`,\s*\{[\s\S]*?requestPriority:\s*options\.requestPriority\s*\?\?\s*"navigation",[\s\S]*?signal:\s*options\.signal\s*\}\)/.test(
    client
  )
) {
  fail("client.ts listFolderAncestors must request the ordered folder path and forward abort signals");
}

if (
  !/listMedia:\s*\(\s*params:\s*ListMediaParams\s*=\s*\{\},\s*options:\s*CoreRequestOptions\s*=\s*\{\}\s*\)\s*=>[\s\S]*?request<Page<MediaRecord>>\(`\/media\$\{query\(params\)\}`,\s*\{[\s\S]*?requestPriority:\s*options\.requestPriority\s*\?\?\s*"navigation",[\s\S]*?signal:\s*options\.signal\s*\}\)/.test(
    client
  )
) {
  fail("client.ts listMedia must serialize typed query params and forward abort signals");
}

if (!/export type ThumbnailPriority = "background" \| "ahead" \| "visible" \| "selected";/.test(generated)) {
  fail("generated-contract.ts must define ThumbnailPriority as the foreground thumbnail priority vocabulary");
}

const thumbnailPriorityScopeSyncBody = interfaceBody("ThumbnailPriorityScopeSyncRequest");
for (const line of [
  "rootId: number;",
  "selectedFileIds: number[];",
  "visibleFileIds: number[];",
  "aheadFileIds: number[];"
]) {
  requireLine(
    thumbnailPriorityScopeSyncBody,
    line,
    "generated-contract.ts ThumbnailPriorityScopeSyncRequest"
  );
}
const thumbnailCacheScopeBody = interfaceBody("ThumbnailCacheScopeParams");
for (const line of ["rootId?: number;", "folderId?: number;", "includeDescendants?: boolean;"]) {
  requireLine(thumbnailCacheScopeBody, line, "generated-contract.ts ThumbnailCacheScopeParams");
}
if (
  !generated.includes(
    'export type ThumbnailCacheRefreshMode =\n  | "missingOnly"\n  | "staleOrMissing"\n  | "retryFailedAndStale";'
  )
) {
  fail("generated-contract.ts must define ThumbnailCacheRefreshMode");
}
const thumbnailCacheTaskBody = interfaceBody("ThumbnailCacheTaskRequest");
for (const line of ["fileIds?: number[];", "refreshMode: ThumbnailCacheRefreshMode;", "limit?: number;"]) {
  requireLine(thumbnailCacheTaskBody, line, "generated-contract.ts ThumbnailCacheTaskRequest");
}
const thumbnailCacheStatsBody = interfaceBody("ThumbnailCacheStatsResponse");
for (const line of [
  "cachedCount: number;",
  "missingCount: number;",
  "staleCount: number;",
  "failedCount: number;",
  "pendingCandidateCount: number;",
  "totalBlobBytes: number;",
  "activeBulkTaskCount: number;"
]) {
  requireLine(thumbnailCacheStatsBody, line, "generated-contract.ts ThumbnailCacheStatsResponse");
}
const thumbnailCacheEnqueueBody = interfaceBody("ThumbnailCacheEnqueueResponse");
requireLine(
  thumbnailCacheEnqueueBody,
  "acceptedCount: number;",
  "generated-contract.ts ThumbnailCacheEnqueueResponse"
);
const thumbnailCacheClearBody = interfaceBody("ThumbnailCacheClearResponse");
for (const line of [
  "cleared: boolean;",
  "deletedBlobCount: number;",
  "resetThumbnailCount: number;"
]) {
  requireLine(thumbnailCacheClearBody, line, "generated-contract.ts ThumbnailCacheClearResponse");
}
if (
  !/getThumbnail:\s*\(\s*fileId:\s*number,\s*target:\s*"grid_320"\s*=\s*"grid_320",\s*priority:\s*ThumbnailPriority\s*=\s*"background",\s*options:\s*CoreRequestOptions\s*=\s*\{\}\s*\)\s*=>[\s\S]*?request<ThumbnailResponse>\(`\/media\/\$\{fileId\}\/thumbnail\$\{query\(\{\s*target,\s*priority\s*}\)\}`,\s*\{[\s\S]*?requestPriority:\s*options\.requestPriority\s*\?\?\s*thumbnailPriorityCoreRequestPriority\(priority\),[\s\S]*?signal:\s*options\.signal\s*\}\)/.test(
    client
  )
) {
  fail("client.ts getThumbnail must send typed thumbnail priority through the query string and forward abort signals");
}

if (!/getThumbnailBlob:\s*async\s*\(\s*fileId:\s*number,\s*target:\s*"grid_320"\s*=\s*"grid_320",\s*options:\s*BlobRequestOptions\s*=\s*{}\s*\)\s*=>\s*\{[\s\S]*fetchBlob\(\s*`\/media\/\$\{fileId\}\/thumbnail\/blob\$\{query\(\{\s*target,\s*v:\s*options\.version\s*}\)\}`,\s*\{[\s\S]*?\.\.\.options,[\s\S]*?requestPriority:\s*options\.requestPriority\s*\?\?\s*"resource"[\s\S]*?\}\s*\)/.test(client)) {
  fail("client.ts getThumbnailBlob must request thumbnail blob with default grid_320 target");
}

if (!/interface\s+BlobRequestOptions[\s\S]*?version\?:\s*number\s*\|\s*string\s*\|\s*null/.test(client)) {
  fail("client.ts blob helpers must expose an optional version cache-buster");
}

if (!/getThumbnailBlob[\s\S]*?query\(\{\s*target,\s*v:\s*options\.version/.test(client)) {
  fail("client.ts getThumbnailBlob must serialize version cache-buster as v");
}

if (!/getPreviewBlob:\s*\(fileId:\s*number,\s*options:\s*BlobRequestOptions\s*=\s*{}\)\s*=>\s*fetchBlob\(`\/media\/\$\{fileId\}\/preview\$\{query\(\{\s*v:\s*options\.version\s*}\)\}`,\s*\{[\s\S]*?\.\.\.options,[\s\S]*?requestPriority:\s*options\.requestPriority\s*\?\?\s*"interactive"[\s\S]*?\}\)/.test(client)) {
  fail("client.ts getPreviewBlob must request original media bytes from /media/{fileId}/preview");
}

if (!/getPreviewResource:\s*\(fileId:\s*number,\s*options:\s*PreviewResourceOptions\s*=\s*\{\}\)\s*=>\s*fetchBlobWithTier\(/.test(client)) {
  fail("client.ts getPreviewResource must request the resolved preview representation");
}
if (!/x-megle-preview-tier/.test(client)) {
  fail("client.ts must read the x-megle-preview-tier response header");
}
// The query serialiser is a whitelist; a parameter it does not know is dropped
// silently and the server never sees a viewport.
if (!/search\.set\("viewportLongPx"/.test(client)) {
  fail("client.ts query() must serialize viewportLongPx");
}

if (!/removeRoot:\s*\(rootId:\s*number\)\s*=>\s*request<AcceptedRootResponse>\(`\/roots\/\$\{rootId\}`,\s*\{[\s\S]*?method:\s*"DELETE",[\s\S]*?requestPriority:\s*"interactive"[\s\S]*?\}\)/.test(client)) {
  fail("client.ts removeRoot must call DELETE /roots/{rootId}");
}

if (!/ScanTaskRequest/.test(client)) {
  fail("client.ts enqueueScan must use the typed ScanTaskRequest body");
}

if (!/enqueueScan:\s*\(rootId:\s*number\)\s*=>\s*request<AcceptedRootResponse>\("\/tasks\/scan",\s*\{[\s\S]*?method:\s*"POST",[\s\S]*?requestPriority:\s*"interactive",[\s\S]*?body:\s*JSON\.stringify\([\s\S]*rootId[\s\S]*\)\s*\}\)/.test(client)) {
  fail("client.ts enqueueScan must call POST /tasks/scan with typed rootId body");
}

if (!/InteractiveFolderScanTaskRequest/.test(client)) {
  fail("client.ts enqueueInteractiveFolderScan must use the typed InteractiveFolderScanTaskRequest body");
}

if (
  !/enqueueInteractiveFolderScan:\s*\(\s*folderId:\s*number,\s*options:\s*CoreRequestOptions\s*=\s*\{\}\s*\)\s*=>\s*request<AcceptedRootResponse>\("\/tasks\/interactive-folder-scan",\s*\{[\s\S]*?method:\s*"POST",[\s\S]*?requestPriority:\s*options\.requestPriority\s*\?\?\s*"background",[\s\S]*?body:\s*JSON\.stringify\([\s\S]*folderId[\s\S]*InteractiveFolderScanTaskRequest[\s\S]*\),[\s\S]*?signal:\s*options\.signal\s*\}\)/.test(
    client
  )
) {
  fail("client.ts enqueueInteractiveFolderScan must call POST /tasks/interactive-folder-scan with typed folderId body and forward abort signals");
}

if (!/ThumbnailPriorityScopeSyncRequest/.test(client)) {
  fail("client.ts syncThumbnailPriorityScope must use the typed ThumbnailPriorityScopeSyncRequest body");
}

if (
  !/syncThumbnailPriorityScope:\s*\(\s*input:\s*ThumbnailPriorityScopeSyncRequest,\s*options:\s*CoreRequestOptions\s*=\s*\{\}\s*\)\s*=>\s*request<AcceptedRootResponse>\("\/tasks\/thumbnail-priority-scope",\s*\{[\s\S]*?method:\s*"POST",[\s\S]*?requestPriority:\s*options\.requestPriority\s*\?\?\s*"interactive",[\s\S]*?body:\s*JSON\.stringify\(input\),[\s\S]*?signal:\s*options\.signal\s*\}\)/.test(
    client
  )
) {
  fail(
    "client.ts syncThumbnailPriorityScope must call POST /tasks/thumbnail-priority-scope with the typed scope payload at interactive priority and forward abort signals"
  );
}
if (
  !/getThumbnailCacheStats:\s*\(\s*params:\s*ThumbnailCacheScopeParams\s*=\s*\{\},\s*options:\s*CoreRequestOptions\s*=\s*\{\}\s*\)\s*=>\s*request<ThumbnailCacheSnapshotResponse>\(`\/thumbnails\/cache\/stats\$\{query\(params\)\}`,\s*\{[\s\S]*?requestPriority:\s*options\.requestPriority\s*\?\?\s*"metadata",[\s\S]*?signal:\s*options\.signal\s*\}\)/.test(
    client
  )
) {
  fail("client.ts getThumbnailCacheStats must serialize typed scope params and forward abort signals");
}
if (
  !/enqueueThumbnailCache:\s*\(\s*input:\s*ThumbnailCacheTaskRequest,\s*options:\s*CoreRequestOptions\s*=\s*\{\}\s*\)\s*=>\s*request<ThumbnailCacheEnqueueResponse>\("\/tasks\/thumbnail-cache",\s*\{[\s\S]*?method:\s*"POST",[\s\S]*?requestPriority:\s*options\.requestPriority\s*\?\?\s*"background",[\s\S]*?body:\s*JSON\.stringify\(input\),[\s\S]*?signal:\s*options\.signal\s*\}\)/.test(
    client
  )
) {
  fail("client.ts enqueueThumbnailCache must call POST /tasks/thumbnail-cache with typed body and forward abort signals");
}
if (
  !/clearThumbnailCache:\s*\(\s*options:\s*CoreRequestOptions\s*=\s*\{\}\s*\)\s*=>\s*request<ThumbnailCacheClearResponse>\("\/thumbnails\/cache\/clear",\s*\{[\s\S]*?method:\s*"POST",[\s\S]*?requestPriority:\s*options\.requestPriority\s*\?\?\s*"interactive",[\s\S]*?signal:\s*options\.signal\s*\}\)/.test(
    client
  )
) {
  fail("client.ts clearThumbnailCache must call POST /thumbnails/cache/clear");
}

if (!/async function readResponseBody\(response:\s*Response\):\s*Promise<unknown>/.test(client)) {
  fail("client.ts must parse response bodies without leaking JSON.parse errors on non-OK responses");
}

if (!/async function readResponseBody[\s\S]*?try\s*{[\s\S]*?JSON\.parse\(text\)[\s\S]*?}\s*catch\s*{[\s\S]*?return text;[\s\S]*?}/.test(client)) {
  fail("client.ts readResponseBody must return raw text for malformed JSON error bodies");
}

if (!/if \(!response\.ok\)\s*{[\s\S]*?const body = await readResponseBody\(response\);[\s\S]*?throw new CoreApiError\(response\.status, body\);[\s\S]*?}\s*const body = await readJson\(response\);/.test(client)) {
  fail("client.ts must use tolerant body parsing only for non-OK CoreApiError responses");
}

if (!client.includes("x-megle-session")) {
  fail("client.ts must send x-megle-session when configured");
}

if (!generated.includes("Generated-client boundary placeholder")) {
  fail("generated-contract.ts must mark the generated-client boundary");
}

if (!process.exitCode) {
  console.log("PASS: core-client contract boundary");
}
