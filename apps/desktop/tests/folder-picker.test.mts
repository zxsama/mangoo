import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { folderPickerDialogOptions, parseFolderPickerRequest, showFolderPicker } from "../src/folder-picker.ts";

describe("managed root folder picker request", () => {
  it("accepts the typed modes and preserves a Core-proposed path", () => {
    assert.deepEqual(parseFolderPickerRequest({
      libraryUuid: "018f26a2-c25e-7e51-9a82-42866f27d504",
      rootUuid: "018f26a2-c25e-7e51-9a82-42866f27d505",
      mode: "relocate",
      suggestedPath: "D:\\Media\\Artists",
      dialogTitle: "选择新的同步位置"
    }), {
      libraryUuid: "018f26a2-c25e-7e51-9a82-42866f27d504",
      rootUuid: "018f26a2-c25e-7e51-9a82-42866f27d505",
      mode: "relocate",
      suggestedPath: "D:\\Media\\Artists",
      dialogTitle: "选择新的同步位置"
    });
  });

  it("rejects unknown modes and malformed identities", () => {
    assert.equal(parseFolderPickerRequest({ libraryUuid: "", rootUuid: null, mode: "relocate", suggestedPath: null, dialogTitle: "x" }), null);
    assert.equal(parseFolderPickerRequest({ libraryUuid: "018f26a2-c25e-7e51-9a82-42866f27d504", rootUuid: null, mode: "delete", suggestedPath: null, dialogTitle: "x" }), null);
    assert.equal(parseFolderPickerRequest({ libraryUuid: "library-a", rootUuid: null, mode: "relocate", suggestedPath: null, dialogTitle: "x" }), null);
  });

  it("validates UUID shape without imposing a UUID version policy beyond Core", () => {
    assert.notEqual(parseFolderPickerRequest({ libraryUuid: "00000000-0000-0000-0000-000000000000", rootUuid: null, mode: "materialize-remote", suggestedPath: null, dialogTitle: "x" }), null);
  });

  it("passes the suggested path and title to the native dialog", () => {
    const request = parseFolderPickerRequest({
      libraryUuid: "018f26a2-c25e-7e51-9a82-42866f27d504",
      rootUuid: null,
      mode: "materialize-remote",
      suggestedPath: "D:\\Media\\Suggested",
      dialogTitle: "选择同步位置"
    });
    assert.deepEqual(folderPickerDialogOptions(request), {
      properties: ["openDirectory"],
      defaultPath: "D:\\Media\\Suggested",
      title: "选择同步位置"
    });
  });

  it("uses the invoking window as owner and returns null on cancellation", async () => {
    const owner = { id: 7 };
    const calls: unknown[][] = [];
    const result = await showFolderPicker(async (...args) => {
      calls.push(args);
      return { canceled: true, filePaths: ["D:\\ignored"] };
    }, owner, null);
    assert.equal(result, null);
    assert.equal(calls.length, 1);
    assert.equal(calls[0]?.[0], owner);
    assert.deepEqual(calls[0]?.[1], { properties: ["openDirectory"] });
  });

  it("supports an ownerless dialog and returns the selected directory", async () => {
    const calls: unknown[][] = [];
    const result = await showFolderPicker(async (...args) => {
      calls.push(args);
      return { canceled: false, filePaths: ["E:\\Chosen"] };
    }, null, null);
    assert.equal(result, "E:\\Chosen");
    assert.deepEqual(calls, [[{ properties: ["openDirectory"] }]]);
  });
});
