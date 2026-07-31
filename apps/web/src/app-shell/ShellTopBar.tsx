import {
  ArrowLeft,
  Bell,
  ChevronsUpDown,
  ChevronLeft,
  ChevronRight,
  Maximize2,
  Minimize2,
  PanelLeftClose,
  PanelLeftOpen,
  Package,
  Settings
} from "lucide-react";
import type * as React from "react";
import type { ComponentProps, ReactNode, RefCallback } from "react";
import { LiquidGlassButton } from "../design/liquid-glass";
import { FilterMenu } from "../features/library/FilterMenu";
import { FolderLocationIcon } from "../features/library/FolderLocationIcon";
import { LayoutMenu } from "../features/library/LayoutMenu";
import { SearchBar } from "../features/library/SearchBar";
import { SortMenu } from "../features/library/SortMenu";
import { LibraryGlyph } from "../features/library/libraryGlyph";
import { type LibraryLayoutMode } from "../features/media-grid/layoutMode";
import { WindowChrome } from "../features/window-chrome/WindowChrome";

type FilterMenuProps = ComponentProps<typeof FilterMenu>;
type SearchBarProps = ComponentProps<typeof SearchBar>;
type SortMenuProps = ComponentProps<typeof SortMenu>;
type PreviewViewMode = "fit-long-edge" | "actual";

interface LibraryTitlebarToolbarProps {
  canNavigateFolderBack: boolean;
  canNavigateFolderForward: boolean;
  favorite: FilterMenuProps["favorite"];
  filterOpen: boolean;
  kind: FilterMenuProps["kind"];
  mediaCount: number;
  minRating: FilterMenuProps["minRating"];
  layoutMode: LibraryLayoutMode;
  layoutOpen: boolean;
  onClearFilters: FilterMenuProps["onClear"];
  onFilterOpenChange: (open: boolean) => void;
  onLayoutModeChange: (mode: LibraryLayoutMode) => void;
  onLayoutOpenChange: (open: boolean) => void;
  onNavigateFolderBack: () => void;
  onNavigateFolderForward: () => void;
  primaryActions?: ReactNode;
  onSetKind: FilterMenuProps["onSetKind"];
  onSetMinRating: FilterMenuProps["onSetMinRating"];
  onSetQ: SearchBarProps["onChange"];
  onSetSort: SortMenuProps["onChange"];
  onSortOpenChange: (open: boolean) => void;
  onToggleFavorite: FilterMenuProps["onToggleFavorite"];
  onToggleTag: FilterMenuProps["onToggleTag"];
  q: SearchBarProps["value"];
  scanActive: boolean;
  searchActive: boolean;
  sort: SortMenuProps["value"];
  sortOpen: boolean;
  sidebarsHidden: boolean;
  shellActions?: ReactNode;
  tagIds: FilterMenuProps["tagIds"];
  tags: FilterMenuProps["tags"];
  title: string;
}

interface PreviewTitlebarToolbarProps {
  canGoNext: boolean;
  canGoPrevious: boolean;
  mode: PreviewViewMode;
  scale: number;
  selectedName: string;
  sidebarsHidden: boolean;
  shellActions?: ReactNode;
  onBack: () => void;
  onGoNext: () => void;
  onGoPrevious: () => void;
  primaryActions?: ReactNode;
  onToggleActualSize: () => void;
}

interface ShellRightActionsProps {
  notificationButtonRef: RefCallback<HTMLButtonElement>;
  notificationIndicator: "idle" | "active" | "failure";
  notificationsOpen: boolean;
  onToggleNotifications: (autoFocus: boolean) => void;
}

export function ShellSidebarToggle({
  sidebarsHidden,
  onToggleSidebars
}: {
  sidebarsHidden: boolean;
  onToggleSidebars: () => void;
}) {
  const Icon = sidebarsHidden ? PanelLeftOpen : PanelLeftClose;
  const label = sidebarsHidden ? "Show sidebars" : "Hide sidebars";

  return (
    <LiquidGlassButton
      aria-label={label}
      aria-pressed={sidebarsHidden}
      className="titlebar-icon-button shell-titlebar-utility-button shell-sidebar-toggle no-drag"
      data-titlebar-control="toggle-sidebars"
      onClick={onToggleSidebars}
      title={`${label} (Tab)`}
      tone="control"
      type="button"
    >
      <Icon aria-hidden="true" size={16} />
    </LiquidGlassButton>
  );
}

export function ShellTitlebarPrimaryActions({
  activeLibraryColor,
  activeLibraryIcon,
  activeLibraryName,
  librarySwitcherButtonRef,
  librarySwitcherOpen,
  sidebarsHidden,
  onOpenLibrarySwitcher,
  onOpenPlugins,
  onOpenSettings,
  onLocateFolder,
  onToggleSidebars
}: {
  activeLibraryColor?: string;
  activeLibraryIcon?: string;
  activeLibraryName: string;
  librarySwitcherButtonRef?: React.Ref<HTMLButtonElement>;
  librarySwitcherOpen: boolean;
  sidebarsHidden: boolean;
  onOpenLibrarySwitcher: (source: HTMLButtonElement, autoFocus: boolean) => void;
  onOpenPlugins: () => void;
  onOpenSettings: () => void;
  onLocateFolder: () => void;
  onToggleSidebars: () => void;
}) {
  return (
    <div
      className="shell-titlebar-primary-actions no-drag"
      data-no-drag="true"
    >
      <LiquidGlassButton
        active={librarySwitcherOpen}
        aria-controls="library-root"
        aria-expanded={librarySwitcherOpen}
        aria-haspopup="dialog"
        aria-label={`Switch library, current library ${activeLibraryName}`}
        className="shell-library-switcher-button no-drag"
        data-titlebar-control="library-switcher"
        onClick={(event) => onOpenLibrarySwitcher(event.currentTarget, event.detail === 0)}
        ref={librarySwitcherButtonRef}
        title="Switch library"
        tone="control"
        type="button"
      >
        <LibraryGlyph icon={activeLibraryIcon} color={activeLibraryColor} size={16} />
        <span className="shell-library-switcher-name">{activeLibraryName}</span>
        <ChevronsUpDown aria-hidden="true" size={14} />
      </LiquidGlassButton>
      <div
        aria-label="Application utilities"
        className="shell-titlebar-utility-group no-drag"
        role="group"
      >
        <LiquidGlassButton
          aria-label="Open plugins"
          className="titlebar-icon-button shell-titlebar-utility-button shell-plugins-button no-drag"
          data-titlebar-control="open-plugins"
          onClick={onOpenPlugins}
          title="Plugins"
          tone="control"
          type="button"
        >
          <Package aria-hidden="true" size={16} />
        </LiquidGlassButton>
        <LiquidGlassButton
          aria-label="Open settings"
          className="titlebar-icon-button shell-titlebar-utility-button shell-settings-button no-drag"
          data-titlebar-control="open-settings"
          onClick={onOpenSettings}
          title="Settings"
          tone="control"
          type="button"
        >
          <Settings aria-hidden="true" size={16} />
        </LiquidGlassButton>
        <div
          aria-hidden="true"
          className="shell-titlebar-utility-separator"
        />
        <LiquidGlassButton
          aria-label="Locate current folder"
          className="titlebar-icon-button shell-titlebar-utility-button shell-locate-folder-button no-drag"
          data-titlebar-control="locate-folder"
          onClick={onLocateFolder}
          title="Locate current folder"
          tone="control"
          type="button"
        >
          <FolderLocationIcon aria-hidden="true" size={16} />
        </LiquidGlassButton>
        <ShellSidebarToggle
          sidebarsHidden={sidebarsHidden}
          onToggleSidebars={onToggleSidebars}
        />
      </div>
    </div>
  );
}

export function LibraryTitlebarToolbar({
  canNavigateFolderBack,
  canNavigateFolderForward,
  favorite,
  filterOpen,
  kind,
  layoutMode,
  layoutOpen,
  mediaCount,
  minRating,
  onClearFilters,
  onFilterOpenChange,
  onLayoutModeChange,
  onLayoutOpenChange,
  onNavigateFolderBack,
  onNavigateFolderForward,
  primaryActions,
  onSetKind,
  onSetMinRating,
  onSetQ,
  onSetSort,
  onSortOpenChange,
  onToggleFavorite,
  onToggleTag,
  q,
  scanActive,
  searchActive,
  shellActions,
  sidebarsHidden,
  sort,
  sortOpen,
  tagIds,
  tags,
  title
}: LibraryTitlebarToolbarProps) {
  return (
    <div
      className={
        shellActions
          ? "titlebar-workspace-toolbar titlebar-library-toolbar has-shell-actions"
          : "titlebar-workspace-toolbar titlebar-library-toolbar"
      }
    >
      {sidebarsHidden ? primaryActions : null}
      <div className="titlebar-workspace-controls titlebar-library-controls no-drag" data-no-drag="true">
        <LiquidGlassButton
          aria-label="Back to previous folder"
          className="titlebar-icon-button library-folder-nav-button"
          data-titlebar-control="library-folder-back"
          disabled={!canNavigateFolderBack}
          onClick={onNavigateFolderBack}
          title="Back"
          tone="control"
          type="button"
        >
          <ChevronLeft aria-hidden="true" size={16} />
        </LiquidGlassButton>
        <LiquidGlassButton
          aria-label="Forward to next folder"
          className="titlebar-icon-button library-folder-nav-button"
          data-titlebar-control="library-folder-forward"
          disabled={!canNavigateFolderForward}
          onClick={onNavigateFolderForward}
          title="Forward"
          tone="control"
          type="button"
        >
          <ChevronRight aria-hidden="true" size={16} />
        </LiquidGlassButton>
        <div className="titlebar-preview-divider" aria-hidden="true" />
        <FilterMenu
          open={filterOpen}
          favorite={favorite}
          kind={kind}
          minRating={minRating}
          onClear={onClearFilters}
          onOpenChange={onFilterOpenChange}
          onSetKind={onSetKind}
          onSetMinRating={onSetMinRating}
          onToggleFavorite={onToggleFavorite}
          onToggleTag={onToggleTag}
          tagIds={tagIds}
          tags={tags}
        />
        <div className="titlebar-tool-title" title="Sort media">
          <SortMenu
            iconOnly
            open={sortOpen}
            value={sort}
            onChange={onSetSort}
            onOpenChange={onSortOpenChange}
          />
        </div>
        <LayoutMenu
          iconOnly
          open={layoutOpen}
          onChange={onLayoutModeChange}
          onOpenChange={onLayoutOpenChange}
          titlebarControlId="library-layout"
          value={layoutMode}
        />
      </div>
      <div className="titlebar-workspace-summary titlebar-library-summary" title={title}>
        {mediaCount} media
        {searchActive ? " / filtered" : ""}
        {scanActive ? " / scanning" : ""}
      </div>
      <div className="titlebar-library-search no-drag" data-no-drag="true">
        <SearchBar value={q} onChange={onSetQ} />
      </div>
      {shellActions ? <div className="titlebar-shell-actions no-drag">{shellActions}</div> : null}
    </div>
  );
}

export function PreviewTitlebarToolbar({
  canGoNext,
  canGoPrevious,
  mode,
  scale,
  shellActions,
  sidebarsHidden,
  selectedName,
  onBack,
  onGoNext,
  onGoPrevious,
  primaryActions,
  onToggleActualSize
}: PreviewTitlebarToolbarProps) {
  const previewScale = `${Math.round(scale * 100)}%`;
  const modeAction = mode === "actual" ? "Fit long edge" : "Actual size";
  const ModeIcon = mode === "actual" ? Maximize2 : Minimize2;

  return (
    <div
      className={
        shellActions
          ? "titlebar-workspace-toolbar titlebar-preview-toolbar has-shell-actions"
          : "titlebar-workspace-toolbar titlebar-preview-toolbar"
      }
    >
      <div className="titlebar-workspace-controls titlebar-preview-controls no-drag" data-no-drag="true">
        {sidebarsHidden ? primaryActions : null}
        <LiquidGlassButton
          aria-label="Back to library"
          className="titlebar-icon-button library-toolbar-back"
          data-titlebar-control="preview-back"
          onClick={onBack}
          title="Back"
          tone="control"
          type="button"
        >
          <ArrowLeft aria-hidden="true" size={16} />
        </LiquidGlassButton>
        <div className="titlebar-preview-divider" aria-hidden="true" />
        <LiquidGlassButton
          aria-label="Previous media"
          className="titlebar-icon-button"
          data-titlebar-control="preview-previous"
          disabled={!canGoPrevious}
          onClick={onGoPrevious}
          title="Previous"
          tone="control"
          type="button"
        >
          <ChevronLeft aria-hidden="true" size={16} />
        </LiquidGlassButton>
        <LiquidGlassButton
          aria-label="Next media"
          className="titlebar-icon-button"
          data-titlebar-control="preview-next"
          disabled={!canGoNext}
          onClick={onGoNext}
          title="Next"
          tone="control"
          type="button"
        >
          <ChevronRight aria-hidden="true" size={16} />
        </LiquidGlassButton>
        <LiquidGlassButton
          aria-label={modeAction}
          aria-pressed={mode === "actual"}
          active={mode === "actual"}
          className="titlebar-icon-button titlebar-preview-mode"
          data-titlebar-control="preview-mode"
          onClick={onToggleActualSize}
          title={modeAction}
          tone="control"
          type="button"
        >
          <ModeIcon aria-hidden="true" size={15} />
        </LiquidGlassButton>
      </div>
      <div
        aria-label={`${selectedName}, ${previewScale}`}
        className="titlebar-workspace-summary titlebar-preview-summary titlebar-preview-title"
        title={selectedName}
      >
        {selectedName}
      </div>
      {shellActions ? (
        <div className="titlebar-shell-actions no-drag">{shellActions}</div>
      ) : (
        <div className="titlebar-preview-spacer" aria-hidden="true" />
      )}
    </div>
  );
}

export function ShellRightActions({
  notificationButtonRef,
  notificationIndicator,
  notificationsOpen,
  onToggleNotifications
}: ShellRightActionsProps) {
  return (
    <div className="shell-right-actions no-drag">
      <LiquidGlassButton
        active={notificationsOpen}
        aria-label="通知"
        aria-pressed={notificationsOpen}
        className="titlebar-icon-button notification-toggle no-drag"
        data-compact-popover-trigger="notifications"
        data-titlebar-control="notifications"
        onClick={(event) => onToggleNotifications(event.detail === 0)}
        ref={notificationButtonRef}
        title="通知"
        tone="control"
        type="button"
      >
        <Bell aria-hidden="true" size={15} />
        {notificationIndicator !== "idle" ? (
          <span
            aria-hidden="true"
            className={`notification-status-dot ${notificationIndicator}`}
          />
        ) : null}
      </LiquidGlassButton>
      <WindowChrome />
    </div>
  );
}
