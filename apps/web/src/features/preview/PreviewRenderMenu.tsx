import { SlidersHorizontal } from "lucide-react";
import { useRef } from "react";

import { LiquidGlassButton } from "../../design/liquid-glass";
import { AnchoredOverlay, OverlayMenuItem, OverlayMenuList } from "../../design/overlays";

import {
  PREVIEW_CHANNELS,
  PREVIEW_RENDER_FILTERS,
  type PreviewChannel,
  type PreviewRenderFilter
} from "./previewRenderSettings";

interface PreviewRenderMenuProps {
  open: boolean;
  filter: PreviewRenderFilter;
  channel: PreviewChannel;
  onChangeFilter: (filter: PreviewRenderFilter) => void;
  onChangeChannel: (channel: PreviewChannel) => void;
  onOpenChange: (open: boolean) => void;
}

// Render controls for the preview surface: which resampling algorithm the
// browser uses when zoomed, and which colour channel is displayed.
//
// Deliberately stateless — App owns the values (they are persisted preferences)
// and the open flag, exactly as it does for SortMenu.
export function PreviewRenderMenu({
  open,
  filter,
  channel,
  onChangeFilter,
  onChangeChannel,
  onOpenChange
}: PreviewRenderMenuProps) {
  const buttonRef = useRef<HTMLButtonElement>(null);

  return (
    <div className="preview-render-menu">
      <LiquidGlassButton
        ref={buttonRef}
        active={open}
        aria-controls="preview-render-root"
        aria-expanded={open}
        aria-haspopup="dialog"
        aria-label="Render settings"
        className="titlebar-icon-button"
        data-titlebar-control="preview-render"
        onClick={() => onOpenChange(!open)}
        title="Render settings"
        tone="control"
        type="button"
      >
        <SlidersHorizontal aria-hidden="true" size={15} />
      </LiquidGlassButton>

      <AnchoredOverlay
        anchor={buttonRef.current}
        ariaLabel="Render settings"
        className="overlay-titlebar-popover preview-render-root"
        id="preview-render-root"
        onDismiss={() => onOpenChange(false)}
        open={open}
        placement="bottom-end"
        preferredWidth={220}
        restoreFocusTarget={buttonRef.current}
        role="group"
        rootId="preview-render"
      >
        {/* Selecting an option deliberately does NOT close the popover: these
            are live render controls, so you change one, look at the image, and
            change another. SortMenu closes because picking a sort is a
            one-shot action. */}
        <div className="preview-render-group">
          <div className="preview-render-group-label">缩放过滤</div>
          <OverlayMenuList ariaLabel="缩放过滤" className="overlay-titlebar-menu" role="listbox">
            {PREVIEW_RENDER_FILTERS.map((option) => {
              const selected = filter === option.value;
              return (
                <OverlayMenuItem
                  aria-selected={selected}
                  checked={selected}
                  key={option.value}
                  onClick={() => onChangeFilter(option.value)}
                  role="option"
                  selected={selected}
                >
                  {option.label}
                </OverlayMenuItem>
              );
            })}
          </OverlayMenuList>
        </div>

        <div className="preview-render-group">
          <div className="preview-render-group-label">通道</div>
          <OverlayMenuList ariaLabel="通道" className="overlay-titlebar-menu" role="listbox">
            {PREVIEW_CHANNELS.map((option) => {
              const selected = channel === option.value;
              return (
                <OverlayMenuItem
                  aria-selected={selected}
                  checked={selected}
                  key={option.value}
                  onClick={() => onChangeChannel(option.value)}
                  role="option"
                  selected={selected}
                >
                  {option.label}
                </OverlayMenuItem>
              );
            })}
          </OverlayMenuList>
        </div>
      </AnchoredOverlay>
    </div>
  );
}
