import type { HTMLAttributes } from "react";
import { LiquidGlassSurface } from "../liquid-glass";
import type { OverlaySurfaceRole } from "./overlayTypes";

export interface OverlaySurfaceProps extends HTMLAttributes<HTMLDivElement> {
  surfaceRole: OverlaySurfaceRole;
  interactive?: boolean;
}

export function OverlaySurface({
  className,
  interactive = false,
  surfaceRole,
  ...props
}: OverlaySurfaceProps) {
  return (
    <LiquidGlassSurface
      {...props}
      as="div"
      className={`overlay-surface overlay-surface-${surfaceRole}${className ? ` ${className}` : ""}`}
      data-overlay-surface-role={surfaceRole}
      interactive={interactive}
      tone="elevated"
    />
  );
}
