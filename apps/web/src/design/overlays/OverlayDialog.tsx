import {
  useLayoutEffect,
  useRef,
  type HTMLAttributes,
  type ReactNode,
  type RefObject
} from "react";
import { createPortal } from "react-dom";
import { useOverlayStack } from "./OverlayProvider";
import { OverlaySurface } from "./OverlaySurface";
import type { OverlayHeaderContent, OverlayRootId } from "./overlayTypes";
import { useOverlayFocusTrap } from "./useOverlayFocusTrap";
import { useOverlayPresence } from "./useOverlayPresence";

export interface ModalSurfaceProps {
  ariaDescribedBy?: string;
  ariaLabelledBy: string;
  busy?: boolean;
  children: ReactNode;
  className?: string;
  id: string;
  initialFocusRef?: RefObject<HTMLElement | null>;
  onDismiss: () => void;
  open: boolean;
  restoreFocusRef?: RefObject<HTMLElement | null>;
  rootId: OverlayRootId;
  role?: "dialog" | "alertdialog";
}

export function ModalSurface({
  ariaDescribedBy,
  ariaLabelledBy,
  busy = false,
  children,
  className,
  id,
  initialFocusRef,
  onDismiss,
  open,
  restoreFocusRef,
  rootId,
  role = "dialog"
}: ModalSurfaceProps) {
  const containerRef = useRef<HTMLElement | null>(null);
  const { registerLayer } = useOverlayStack();
  const presence = useOverlayPresence(open, "modal");

  useLayoutEffect(() => {
    containerRef.current = presence.present ? document.getElementById(id) : null;
    return () => {
      containerRef.current = null;
    };
  }, [id, presence.present]);

  useOverlayFocusTrap(open && presence.present, containerRef, {
    initialFocusRef,
    restoreFocusRef
  });

  useLayoutEffect(() => {
    if (!open || !presence.present || !containerRef.current) return;
    return registerLayer({
      id,
      rootId,
      kind: "modal",
      trigger: restoreFocusRef?.current ?? null,
      surface: containerRef.current,
      dismissOnEscape: !busy,
      dismissOnFocusOutside: false,
      dismissOnOutsidePointer: !busy,
      dismissOnWindowBlur: false,
      restoreFocus: false,
      onDismiss
    });
  }, [
    busy,
    id,
    onDismiss,
    open,
    presence.present,
    registerLayer,
    restoreFocusRef,
    rootId
  ]);

  if (!presence.present) return null;

  return createPortal(
    <div
      className="overlay-modal-backdrop"
      aria-hidden={!open || undefined}
      data-overlay-layer-kind="modal"
      data-overlay-state={presence.phase}
    >
      <OverlaySurface
        aria-busy={busy || undefined}
        aria-describedby={ariaDescribedBy}
        aria-labelledby={ariaLabelledBy}
        aria-modal="true"
        className={`overlay-modal-surface${className ? ` ${className}` : ""}`}
        data-overlay-layer-kind="modal"
        data-overlay-state={presence.phase}
        id={id}
        role={role}
        surfaceRole="modal"
        tabIndex={-1}
      >
        {children}
      </OverlaySurface>
    </div>,
    document.body
  );
}

export function OverlayDialogHeader({
  description,
  descriptionId,
  title,
  titleId
}: OverlayHeaderContent) {
  return (
    <header className="overlay-dialog-header">
      <h2 id={titleId}>{title}</h2>
      {description ? <p id={descriptionId}>{description}</p> : null}
    </header>
  );
}

export function OverlayDialogBody({ className, ...props }: HTMLAttributes<HTMLDivElement>) {
  return (
    <div
      {...props}
      className={`overlay-dialog-body${className ? ` ${className}` : ""}`}
    />
  );
}

export function OverlayDialogActions({ className, ...props }: HTMLAttributes<HTMLElement>) {
  return (
    <footer
      {...props}
      className={`overlay-dialog-actions${className ? ` ${className}` : ""}`}
    />
  );
}

export function OverlayInlineStatus({
  children,
  tone
}: {
  children: ReactNode;
  tone: "error" | "warning" | "info";
}) {
  return (
    <div
      className="overlay-inline-status"
      data-tone={tone}
      role={tone === "error" ? "alert" : "status"}
    >
      {children}
    </div>
  );
}
