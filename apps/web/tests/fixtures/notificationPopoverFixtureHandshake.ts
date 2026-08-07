export type NotificationPopoverFixtureState =
  | "full"
  | "scan"
  | "thumbnail"
  | "failure"
  | "empty"
  | "degraded";

export interface NotificationPopoverFixtureLifecycle {
  ready: boolean;
  state: NotificationPopoverFixtureState | null;
  requestedOpenId: number;
  renderedOpenId: number;
  open: boolean;
}

export function createNotificationPopoverFixtureHandshake() {
  let ready = false;
  let state: NotificationPopoverFixtureState | null = null;
  let requestedOpenId = 0;
  let renderedOpenId = 0;
  let open = false;

  return {
    markReady(nextState: NotificationPopoverFixtureState) {
      ready = true;
      state = nextState;
    },
    requestOpen() {
      requestedOpenId += 1;
      open = false;
      return requestedOpenId;
    },
    markRenderedOpen(requestId: number) {
      if (requestId !== requestedOpenId) {
        throw new Error(
          `notification fixture rendered stale open request ${requestId}; expected ${requestedOpenId}`
        );
      }
      renderedOpenId = requestId;
      open = true;
    },
    markClosed() {
      open = false;
    },
    snapshot(): NotificationPopoverFixtureLifecycle {
      return { ready, state, requestedOpenId, renderedOpenId, open };
    }
  };
}
