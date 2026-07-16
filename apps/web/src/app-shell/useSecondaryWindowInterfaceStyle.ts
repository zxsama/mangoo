import { useEffect, useRef } from "react";
import { broadcastPreference, subscribePreference } from "../core/preferenceSync";
import {
  normalizeInterfaceStyle,
  useInterfaceStyle,
  type InterfaceStyleController
} from "../design/liquid-glass";

export function useSecondaryWindowInterfaceStyle({
  broadcastChanges = false
}: {
  broadcastChanges?: boolean;
} = {}): InterfaceStyleController {
  const interfaceStyle = useInterfaceStyle();
  const lastObservedValue = useRef(JSON.stringify(interfaceStyle.value));

  useEffect(
    () =>
      subscribePreference("interfaceStyle", (value) => {
        const normalized = normalizeInterfaceStyle(value);
        lastObservedValue.current = JSON.stringify(normalized);
        interfaceStyle.setInterfaceStyle(normalized);
      }),
    [interfaceStyle.setInterfaceStyle]
  );

  useEffect(() => {
    const serialized = JSON.stringify(interfaceStyle.value);
    if (serialized === lastObservedValue.current) return;
    lastObservedValue.current = serialized;
    if (broadcastChanges) {
      broadcastPreference("interfaceStyle", interfaceStyle.value);
    }
  }, [broadcastChanges, interfaceStyle.value]);

  return interfaceStyle;
}
