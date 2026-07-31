import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { describe, it } from "node:test";
import { REMOVE_ROOT_CONFIRM } from "../src/core/removeRootConfirm.ts";

const switcherSource = readFileSync(
  new URL("../src/features/library/LibrarySwitcher.tsx", import.meta.url),
  "utf8",
);

describe("REMOVE_ROOT_CONFIRM", () => {
  it("names the permanently-lost data types and reassures about disk files", () => {
    assert.match(REMOVE_ROOT_CONFIRM.body, /评分/);
    assert.match(REMOVE_ROOT_CONFIRM.body, /标签/);
    assert.match(REMOVE_ROOT_CONFIRM.body, /备注/);
    assert.match(REMOVE_ROOT_CONFIRM.body, /永久丢失|无法恢复/);
    assert.match(REMOVE_ROOT_CONFIRM.body, /磁盘.*不受影响|原文件不受影响/);
    assert.equal(REMOVE_ROOT_CONFIRM.confirmLabel, "移除");
    assert.equal(REMOVE_ROOT_CONFIRM.cancelLabel, "取消");
    assert.equal(REMOVE_ROOT_CONFIRM.title("我的图库"), "从库移除「我的图库」?");
  });

  it("keeps cancellation inside the library root and initially focuses it", () => {
    assert.match(switcherSource, /<ModalSurface[^]*initialFocusRef=\{cancelRef\}/);
    assert.match(switcherSource, /busy=\{removePending\}/);
    assert.match(switcherSource, /open=\{removeTarget !== null\}/);
    assert.match(switcherSource, /restoreFocusRef=\{removeTriggerRef\}/);

    const dialogActions = switcherSource.match(
      /<OverlayDialogActions>([^]*?)<\/OverlayDialogActions>/,
    )?.[1];
    assert.ok(dialogActions);
    assert.match(
      dialogActions,
      /<button[^]*?disabled=\{removePending\}[^]*?onClick=\{\(\) => dispatch\(\{ type: "cancel-remove" \}\)\}[^]*?ref=\{cancelRef\}/,
    );
    assert.doesNotMatch(dialogActions, /onClose/);
  });
});
