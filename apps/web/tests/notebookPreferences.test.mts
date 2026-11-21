import assert from "node:assert/strict";
import { test } from "node:test";
import { mergeNoteEditorStyles } from "../../../bundled-plugins/com.mangoo.notebook/src/web/editor/noteFormat.ts";
import {
  defaultNotebookPreferences, normalizeNotebookPreferences, readNotebookPreferences, resetNotebookPreferences,
  subscribeNotebookPreferences, updateNotebookPreferences
} from "../../../bundled-plugins/com.mangoo.notebook/src/web/notebookPreferences.ts";

test("device defaults cascade per element and per property, including custom declarations", () => {
  const defaults = normalizeNotebookPreferences({
    markdownMode: "unrecognized", htmlMode: "preview", outlineOpen: "false", wrapSource: false,
    elementStyles: {
      body: { color: "#224466", fontSize: 20, fontWeight: 9000, customCss: "Color: navy; padding: 5px" },
      image: { width: 50 }, unused: { fontSize: 28 }
    }
  });
  assert.deepEqual(defaults, {
    ...defaultNotebookPreferences, htmlMode: "preview", wrapSource: false,
    elementStyles: { body: { color: "#224466", fontSize: 20, customCss: "Color: navy; padding: 5px" }, image: { width: 50 } }
  });
  const local = { body: { color: "#ffffff", customCss: "font-style: italic" }, h1: { fontSize: 30 } };
  const result = mergeNoteEditorStyles(defaults.elementStyles, local);
  assert.equal(result.body?.fontSize, 20);
  assert.equal(result.body?.color, "#ffffff");
  assert.deepEqual(result.body?.customCss?.split(";").map(value => value.trim()), ["padding: 5px", "font-style: italic"]);
  assert.deepEqual(result.image, { width: 50 });
  assert.deepEqual(result.h1, { fontSize: 30 });
  assert.deepEqual(mergeNoteEditorStyles(defaults.elementStyles, {}), defaults.elementStyles);
  assert.equal(local.body.customCss, "font-style: italic");
  assert.equal(defaults.elementStyles.body?.customCss, "Color: navy; padding: 5px");
});

test("notebook preferences persist patches, signal local and peer changes, and recover from malformed data", () => {
  const original = Object.getOwnPropertyDescriptor(globalThis, "window");
  let raw: string | null = null;
  let key = "";
  let storageFull = false;
  const target = Object.assign(new EventTarget(), { localStorage: {
    getItem: () => raw,
    setItem(name: string, value: string) {
      if (storageFull) throw new Error("Storage full");
      key = name;
      raw = value;
    }
  } });
  Object.defineProperty(globalThis, "window", { configurable: true, value: target });
  const seen: string[] = [];
  const unsubscribe = subscribeNotebookPreferences(() => seen.push(readNotebookPreferences().markdownMode));
  try {
    updateNotebookPreferences({ outlineOpen: true });
    updateNotebookPreferences({ markdownMode: "source" });
    assert.equal(JSON.parse(raw!).outlineOpen, true);
    assert.deepEqual(seen, ["live", "source"]);
    assert.equal(readNotebookPreferences(), readNotebookPreferences());
    raw = JSON.stringify({ markdownMode: "live", htmlMode: "code" });
    target.dispatchEvent(Object.assign(new Event("storage"), { key }));
    assert.equal(seen.at(-1), "live");
    assert.equal(readNotebookPreferences().htmlMode, "code");
    raw = "[broken";
    assert.deepEqual(readNotebookPreferences(), defaultNotebookPreferences);
    storageFull = true;
    updateNotebookPreferences({ wrapSource: false });
    assert.equal(readNotebookPreferences().wrapSource, false);
    storageFull = false;
    resetNotebookPreferences();
    assert.deepEqual(JSON.parse(raw), defaultNotebookPreferences);
  } finally {
    unsubscribe();
    if (original) Object.defineProperty(globalThis, "window", original);
    else Reflect.deleteProperty(globalThis, "window");
  }
});
