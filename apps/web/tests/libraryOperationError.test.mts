import assert from "node:assert/strict";
import { it } from "node:test";
import { CoreApiError } from "@mangoo/core-client";
import { libraryOperationError } from "../src/features/library/libraryOperationError.ts";

it("translates actual Core error/code payloads into actionable Library recovery messages", () => {
  const cases = [
    ["library_marker_not_found", "不是 Mangoo 资源库"],
    ["invalid_library_marker", "资源库信息无效"],
    ["unsupported_library_marker", "格式版本暂不受支持"],
    ["library_marker_not_committed", "尚未完成创建或恢复"],
    ["invalid_path", "位置无效"],
    ["library_path_not_found", "找不到此文件夹"],
    ["library_path_unavailable_or_reparse", "检查磁盘连接和访问权限"],
    ["library_identity_conflict", "身份与已登记的位置不一致"],
    ["library_local_data_conflict", "本机已有不同的资源库数据"],
    ["library_local_data_unavailable", "本机目录数据无法读取"],
    ["nested_library_root", "位置与已有资源库或应用数据目录重叠"],
    ["library_open_conflict", "另一项打开操作"],
    ["library_not_found", "资源库已不在列表中"],
    ["library_unavailable", "资源库暂不可用"],
    ["path_changed", "资源库位置已变化"],
    ["stale_context_revision", "活动资源库已改变"],
    ["host_shutting_down", "应用正在关闭"],
    ["library_stop_failed: worker_unavailable", "暂时无法关闭当前资源库"],
  ];
  for (const [code, expected] of cases) {
    const message = libraryOperationError(new CoreApiError(409, { code, error: code.replaceAll("_", "-") }));
    assert.ok(message.includes(expected), `${code}: ${message}`);
  }
});

it("preserves unknown causes and handles a bodyless not-found response without raw HTTP prose", () => {
  assert.equal(libraryOperationError(new CoreApiError(503, { code: "future_error", error: "Diagnostic detail" })), "Diagnostic detail");
  assert.equal(libraryOperationError(new CoreApiError(500, { message: "Older adapter detail" })), "Older adapter detail");
  assert.match(libraryOperationError(new CoreApiError(404, null)), /重新选择已有 Mangoo 资源库文件夹/);
  assert.equal(libraryOperationError(new Error("Unknown failure")), "Unknown failure");
  assert.equal(libraryOperationError("Cancelled by service"), "Cancelled by service");
});

it("gives a readable connection retry message for browser fetch failures", () => {
  for (const message of ["Failed to fetch", "Load failed", "NetworkError when attempting to fetch resource."]) {
    assert.equal(libraryOperationError(new TypeError(message)), "无法连接资源库服务，请检查连接后重试。");
  }
  assert.equal(libraryOperationError(new TypeError("Unexpected metadata value")), "Unexpected metadata value");
});
