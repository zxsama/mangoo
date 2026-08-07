import assert from "node:assert/strict";
import test from "node:test";

import { createNotificationPopoverFixtureHandshake } from "./fixtures/notificationPopoverFixtureHandshake.ts";

test("notification fixture handshake distinguishes ready, requested, and rendered open state", () => {
  const handshake = createNotificationPopoverFixtureHandshake();

  assert.deepEqual(handshake.snapshot(), {
    ready: false,
    state: null,
    requestedOpenId: 0,
    renderedOpenId: 0,
    open: false
  });

  handshake.markReady("failure");
  const requestId = handshake.requestOpen();
  assert.equal(requestId, 1);
  assert.deepEqual(handshake.snapshot(), {
    ready: true,
    state: "failure",
    requestedOpenId: 1,
    renderedOpenId: 0,
    open: false
  });

  handshake.markRenderedOpen(requestId);
  assert.equal(handshake.snapshot().open, true);
  assert.equal(handshake.snapshot().renderedOpenId, requestId);
});

test("notification fixture handshake supports deterministic reopen after dismissal", () => {
  const handshake = createNotificationPopoverFixtureHandshake();
  handshake.markReady("full");
  const firstRequest = handshake.requestOpen();
  handshake.markRenderedOpen(firstRequest);
  handshake.markClosed();

  const secondRequest = handshake.requestOpen();
  assert.equal(secondRequest, firstRequest + 1);
  assert.equal(handshake.snapshot().open, false);
  handshake.markRenderedOpen(secondRequest);
  assert.deepEqual(handshake.snapshot(), {
    ready: true,
    state: "full",
    requestedOpenId: secondRequest,
    renderedOpenId: secondRequest,
    open: true
  });
});
