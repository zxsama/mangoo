import type { createCoreClient } from "@mangoo/core-client";
import { array, boolean, integer, nullable, numericPage, object, oneOf, optional, options, record, stringPage, text, tuple, type Rule } from "./pluginUiValidation.ts";

export type PluginUiCoreClient = ReturnType<typeof createCoreClient>;
interface Entry { validate: (args: unknown[]) => boolean; invoke: (args: unknown[]) => Promise<unknown>; }

const mutation = { operationUuid: text, expectedOperationRevision: integer, expectedBindingGeneration: optional(nullable(integer)) };
const fence = object({ ...mutation, expectedBindingGeneration: integer });
const selection = object({ ...mutation, rootUuid: text, selected: boolean, path: text });
const rename = { ...mutation, requestedName: text, expectedLocationHeads: array(text), supersedes: array(text) };
const relocate = { ...mutation, expectedBindingGeneration: integer, mode: oneOf("explicit-move", "automatic-rename"), targetPath: text };
const restoreRoot = object({ rootUuid: text, expectedLogicalRevision: integer, expectedMaterializationFingerprint: text, selected: boolean, targetPath: optional(text) });
const restoreItem: Rule = (v) => object({ restoreMode: oneOf("root-aware"), accountUuid: text, libraryUuid: text, placementBase: text, inventoryFence: text, roots: array(restoreRoot) })(v)
  || object({ restoreMode: optional(oneOf("legacy-library")), accountUuid: text, libraryUuid: text, name: text, placementBase: text })(v);
const command = object({ operationUuid: text });
const externalLink = object({ kind: oneOf("authorization", "help"), id: text });

/** Explicit transport adapters only; feature sequencing belongs to the plugin document. */
export function pluginUiCoreMethods(c: PluginUiCoreClient, contextRevision: number) {
  function expose<F extends (...args: never[]) => Promise<unknown>>(call: F, rules: Rule[], revisionIndex?: number): Entry {
    const validate = (args: unknown[]) => tuple(rules, args) && (revisionIndex === undefined || args[revisionIndex] === contextRevision);
    return { validate, invoke: (args) => {
      if (!validate(args)) throw new Error("Invalid plugin method arguments or Library context revision");
      const pinned = [...args];
      if (revisionIndex !== undefined) pinned[revisionIndex] = contextRevision;
      // Only the explicit typed endpoint below receives the session-owned mutation revision.
      return call(...pinned as Parameters<F>);
    } };
  }
  return {
    getActiveLibrary: expose(c.getActiveLibrary, [options]),
    listLibraries: expose(c.listLibraries, [options]),
    listRoots: expose(c.listRoots, []),
    preflightLibrary: expose(c.preflightLibrary, [object({ name: text, databasePath: text, contentRoots: array(object({ path: text, createIfMissing: boolean })) }), options]),
    removeLocalLibrary: expose(c.removeLocalLibrary, [text, integer, options], 1),
    getPlugin: expose(c.getPlugin, [text]),
    getPluginConfiguration: expose(c.getPluginConfiguration, [text]),
    beginPluginAuth: expose(c.beginPluginAuth, [text]),
    completePluginAuth: expose(c.completePluginAuth, [text, object({ flowId: text, responseUri: text })]),
    cancelPluginAuth: expose(c.cancelPluginAuth, [text, object({ flowId: text })]),
    signOutPlugin: expose(c.signOutPlugin, [text]),
    resolvePluginExternalLink: expose(c.resolvePluginExternalLink, [text, externalLink]),
    planPluginConfiguration: expose(c.planPluginConfiguration, [text, object({ expectedRevision: integer, values: (v) => record(v) && Object.values(v).every((x) => x === null || typeof x === "string") })]),
    commitPluginConfiguration: expose(c.commitPluginConfiguration, [text, object({ planUuid: text, expectedRevision: integer, confirmed: boolean })]),
    getSyncStatus: expose(c.getSyncStatus, [options]),
    listRemoteLibraries: expose(c.listRemoteLibraries, [options]),
    listRemoteLibraryRoots: expose(c.listRemoteLibraryRoots, [text, text, stringPage, options]),
    preflightSyncRestore: expose(c.preflightSyncRestore, [object({ items: array(restoreItem) })]),
    startSyncRestore: expose(c.startSyncRestore, [object({ items: array(restoreItem), operationUuid: text, preflightToken: text })]),
    getSyncRestore: expose(c.getSyncRestore, [text, options]),
    pauseSyncRestore: expose(c.pauseSyncRestore, [text]),
    resumeSyncRestore: expose(c.resumeSyncRestore, [text]),
    cancelSyncRestore: expose(c.cancelSyncRestore, [text]),
    preflightLibrarySync: expose(c.preflightLibrarySync, [text, object({ providerId: text, accountUuid: text, remoteLibraryUuid: nullable(text) }), options]),
    startLibrarySyncEnable: expose(c.startLibrarySyncEnable, [text, integer, object({ operationUuid: text, providerId: text, accountUuid: text, preflightToken: text }), options], 1),
    pauseLibrarySync: expose(c.pauseLibrarySync, [text, integer, command], 1),
    resumeLibrarySync: expose(c.resumeLibrarySync, [text, integer, command], 1),
    retryLibrarySync: expose(c.retryLibrarySync, [text, integer, command], 1),
    syncLibraryNow: expose(c.syncLibraryNow, [text, integer, command], 1),
    planLibrarySyncUnbind: expose(c.planLibrarySyncUnbind, [text]),
    commitLibrarySyncUnbind: expose(c.commitLibrarySyncUnbind, [text, integer, object({ operationUuid: text, planUuid: text, choice: oneOf("drain-before-unbind", "keep-local-fork") })], 1),
    preflightLibrarySyncReconnect: expose(c.preflightLibrarySyncReconnect, [text, object({ accountUuid: text, remoteLibraryUuid: text })]),
    commitLibrarySyncReconnect: expose(c.commitLibrarySyncReconnect, [text, integer, object({ operationUuid: text, planUuid: text, choice: oneOf("merge-local-fork", "publish-new-remote") })], 1),
    listLibrarySyncConflicts: expose(c.listLibrarySyncConflicts, [text, stringPage, options]),
    resolveLibrarySyncConflict: expose(c.resolveLibrarySyncConflict, [text, text, integer, object({ operationUuid: text, resolution: oneOf("resolve", "ignore") })], 2),
    getLibrarySyncDiagnostics: expose(c.getLibrarySyncDiagnostics, [text]),
    exportLibrarySyncDiagnostics: expose(c.exportLibrarySyncDiagnostics, [text]),
    listSyncFolders: expose(c.listSyncFolders, [text, options]),
    getSyncFolder: expose(c.getSyncFolder, [text, text, options]),
    preflightSyncFolderEnrollment: expose(c.preflightSyncFolderEnrollment, [text, object({ ...mutation, rootUuid: optional(nullable(text)), mode: oneOf("enroll-existing"), path: text }), options]),
    commitSyncFolderEnrollment: expose(c.commitSyncFolderEnrollment, [text, integer, object({ operationUuid: text, expectedOperationRevision: integer, rootUuid: text, rootFolderUuid: text, preflightToken: text, path: text, requestedName: text }), options], 1),
    preflightSyncFolderSelection: expose(c.preflightSyncFolderSelection, [text, object({ items: array(selection) }), options]),
    commitSyncFolderSelection: expose(c.commitSyncFolderSelection, [text, integer, object({ preflightToken: text, items: array(selection) }), options], 1),
    commandSyncFolder: expose(c.commandSyncFolder, [text, text, oneOf("pause", "resume", "stop-sync"), integer, fence, options], 3),
    preflightSyncFolderStop: expose(c.preflightSyncFolderStop, [text, text, fence, options]),
    commitSyncFolderStop: expose(c.commitSyncFolderStop, [text, text, integer, object({ ...mutation, expectedBindingGeneration: integer, planUuid: text }), options], 2),
    preflightSyncFolderRelocation: expose(c.preflightSyncFolderRelocation, [text, text, object(relocate), options]),
    startSyncFolderRelocation: expose(c.startSyncFolderRelocation, [text, text, integer, object({ ...relocate, preflightToken: text, desiredMaterializationFingerprint: text }), options], 2),
    getSyncFolderRelocation: expose(c.getSyncFolderRelocation, [text, text, text, options]),
    commandSyncFolderRelocation: expose(c.commandSyncFolderRelocation, [text, text, text, oneOf("cancel", "retry"), integer, fence, options], 4),
    preflightSyncFolderRename: expose(c.preflightSyncFolderRename, [text, text, object(rename), options]),
    commitSyncFolderRename: expose(c.commitSyncFolderRename, [text, text, integer, object({ ...rename, planUuid: text }), options], 2),
    preflightSyncFolderRebind: expose(c.preflightSyncFolderRebind, [text, text, object({ ...mutation, path: text, markerIdentity: text, priorCutoverSnapshotUuid: text }), options]),
    commitSyncFolderRebind: expose(c.commitSyncFolderRebind, [text, text, integer, object({ ...mutation, planUuid: text }), options], 2),
    preflightSyncFolderReenable: expose(c.preflightSyncFolderReenable, [text, text, fence, options]),
    commitSyncFolderReenable: expose(c.commitSyncFolderReenable, [text, text, integer, object({ ...mutation, expectedBindingGeneration: integer, planUuid: text, mode: oneOf("merge-local-fork", "publish-as-new") }), options], 2),
    getSyncFolderComparison: expose(c.getSyncFolderComparison, [text, text, text, options]),
    listSyncFolderComparisonItems: expose(c.listSyncFolderComparisonItems, [text, text, text, numericPage, options]),
    resolveSyncFolderComparisonItem: expose(c.resolveSyncFolderComparisonItem, [text, text, text, integer, integer, object({ ...mutation, expectedBindingGeneration: integer, resolution: oneOf("keep-local", "keep-remote", "keep-both") }), options], 4),
    listSyncFolderConflicts: expose(c.listSyncFolderConflicts, [text, text, stringPage, options]),
    resolveSyncFolderConflict: expose(c.resolveSyncFolderConflict, [text, text, text, integer, object({ ...mutation, expectedLocationHeads: array(text), supersedes: array(text), requestedName: optional(nullable(text)) }), options], 3),
  };
}

export type PluginUiCoreMethod = keyof ReturnType<typeof pluginUiCoreMethods>;
