import {
  ArrowLeft,
  ChevronLeft,
  ChevronRight,
  History,
  Images,
  ListChecks,
  Maximize2,
  Minimize2,
  PanelLeftClose,
  PanelLeftOpen,
  Package,
  Settings
} from "lucide-react";
import type { LucideIcon } from "lucide-react";
import type { ComponentProps, ReactNode } from "react";
import { LiquidGlassButton } from "../design/liquid-glass";
import { FilterMenu } from "../features/library/FilterMenu";
import { LayoutMenu } from "../features/library/LayoutMenu";
import { SearchBar } from "../features/library/SearchBar";
import { SortMenu } from "../features/library/SortMenu";
import { type LibraryLayoutMode } from "../features/media-grid/layoutMode";
import { WindowChrome } from "../features/window-chrome/WindowChrome";

export type ShellWorkspaceView = "library" | "plugins";

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
  onClearFilters: FilterMenuProps["onClear"];
  onFilterOpenChange: (open: boolean) => void;
  onLayoutModeChange: (mode: LibraryLayoutMode) => void;
  onNavigateFolderBack: () => void;
  onNavigateFolderForward: () => void;
  onOpenSettings: () => void;
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
  onToggleSidebars: () => void;
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
  onOpenSettings: () => void;
  onToggleSidebars: () => void;
  onToggleActualSize: () => void;
}

interface ShellRightActionsProps {
  recentOpsOpen: boolean;
  scanActive: boolean;
  taskPaletteOpen: boolean;
  onCloseTasks: () => void;
  onOpenTasks: () => void;
  onToggleRecent: () => void;
}

const tabs = [
  { id: "library", label: "Library", caption: "Library", icon: Images },
  { id: "plugins", label: "Plugins", caption: "Plugins", icon: Package }
] satisfies Array<{
  id: ShellWorkspaceView;
  label: string;
  caption: string;
  icon: LucideIcon;
}>;

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

function ShellTitlebarUtilities({
  sidebarsHidden,
  onOpenSettings,
  onToggleSidebars
}: {
  sidebarsHidden: boolean;
  onOpenSettings: () => void;
  onToggleSidebars: () => void;
}) {
  return (
    <div
      className="shell-titlebar-utilities no-drag"
      data-no-drag="true"
      data-pointer-glow="off"
    >
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
      <ShellSidebarToggle
        sidebarsHidden={sidebarsHidden}
        onToggleSidebars={onToggleSidebars}
      />
    </div>
  );
}

export function ShellPrimaryNav({
  activeView,
  onOpenSettings,
  onToggleSidebars,
  onSelectView
}: {
  activeView: ShellWorkspaceView;
  onOpenSettings: () => void;
  onToggleSidebars: () => void;
  onSelectView: (view: ShellWorkspaceView) => void;
}) {
  return (
    <div
      className="shell-primary-nav-row no-drag"
      data-no-drag="true"
      data-pointer-glow="off"
    >
      <nav className="shell-primary-nav no-drag" aria-label="Workbench sections" role="tablist">
        {tabs.map((tab) => {
          const Icon = tab.icon;
          return (
            <LiquidGlassButton
              active={activeView === tab.id}
              aria-current={activeView === tab.id ? "page" : undefined}
              aria-label={tab.label}
              aria-selected={activeView === tab.id}
              className={
                activeView === tab.id
                  ? "shell-nav-button active no-drag"
                  : "shell-nav-button no-drag"
              }
              data-titlebar-control={`nav-${tab.id}`}
              key={tab.id}
              onClick={() => onSelectView(tab.id)}
              role="tab"
              title={tab.label}
              tone="control"
              type="button"
            >
              <Icon aria-hidden="true" size={17} />
              <span className="shell-nav-caption">{tab.caption}</span>
            </LiquidGlassButton>
          );
        })}
      </nav>
      <ShellTitlebarUtilities
        sidebarsHidden={false}
        onOpenSettings={onOpenSettings}
        onToggleSidebars={onToggleSidebars}
      />
    </div>
  );
}

export function ShellTitlebarPlaceholder({
  label,
  onOpenSettings,
  onToggleSidebars,
  shellActions,
  sidebarsHidden = false
}: {
  label?: string;
  onOpenSettings?: () => void;
  onToggleSidebars?: () => void;
  shellActions?: ReactNode;
  sidebarsHidden?: boolean;
}) {
  if (!label) {
    return <div className="shell-titlebar-placeholder" aria-hidden="true" />;
  }

  if (!sidebarsHidden || !onToggleSidebars || !onOpenSettings) {
    return <div className="shell-titlebar-summary">{label}</div>;
  }

  return (
    <div className="titlebar-workspace-toolbar titlebar-placeholder-toolbar has-shell-actions">
      <ShellTitlebarUtilities
        sidebarsHidden
        onOpenSettings={onOpenSettings}
        onToggleSidebars={onToggleSidebars}
      />
      <div className="shell-titlebar-summary">{label}</div>
      {shellActions ? <div className="titlebar-shell-actions no-drag">{shellActions}</div> : null}
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
  mediaCount,
  minRating,
  onClearFilters,
  onFilterOpenChange,
  onLayoutModeChange,
  onNavigateFolderBack,
  onNavigateFolderForward,
  onOpenSettings,
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
  title,
  onToggleSidebars
}: LibraryTitlebarToolbarProps) {
  return (
    <div
      className={
        shellActions
          ? "titlebar-workspace-toolbar titlebar-library-toolbar has-shell-actions"
          : "titlebar-workspace-toolbar titlebar-library-toolbar"
      }
    >
      {sidebarsHidden ? (
        <ShellTitlebarUtilities
          sidebarsHidden
          onOpenSettings={onOpenSettings}
          onToggleSidebars={onToggleSidebars}
        />
      ) : null}
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
          onChange={onLayoutModeChange}
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
  onOpenSettings,
  onToggleSidebars,
  onToggleActualSize
}: PreviewTitlebarToolbarProps) {
  const previewScale = `${Math.round(scale * 100)}%`;
  const modeAction = mode === "actual" ? "Fit long edge" : "Actual size";
  const ModeIcon = mode === "actual" ? Minimize2 : Maximize2;

  return (
    <div
      className={
        shellActions
          ? "titlebar-workspace-toolbar titlebar-preview-toolbar has-shell-actions"
          : "titlebar-workspace-toolbar titlebar-preview-toolbar"
      }
    >
      <div className="titlebar-workspace-controls titlebar-preview-controls no-drag" data-no-drag="true">
        {sidebarsHidden ? (
          <ShellTitlebarUtilities
            sidebarsHidden
            onOpenSettings={onOpenSettings}
            onToggleSidebars={onToggleSidebars}
          />
        ) : null}
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
  recentOpsOpen,
  scanActive,
  taskPaletteOpen,
  onCloseTasks,
  onOpenTasks,
  onToggleRecent
}: ShellRightActionsProps) {
  return (
    <div className="shell-right-actions no-drag">
      <LiquidGlassButton
        active={taskPaletteOpen || scanActive}
        aria-label={taskPaletteOpen ? "Close tasks palette" : "Open tasks palette"}
        aria-pressed={taskPaletteOpen}
        className={
          taskPaletteOpen || scanActive
            ? "top-action task-drawer-toggle active no-drag"
            : "top-action task-drawer-toggle no-drag"
        }
        data-compact-popover="tasks"
        data-compact-popover-trigger="tasks"
        data-titlebar-control="tasks-palette"
        onClick={taskPaletteOpen ? onCloseTasks : onOpenTasks}
        title={taskPaletteOpen ? "Close tasks palette" : "Open tasks palette"}
        tone="control"
        type="button"
      >
        <ListChecks size={16} />
        <span className="top-action-label">Tasks</span>
        {scanActive ? <span className="task-drawer-status" aria-hidden="true" /> : null}
      </LiquidGlassButton>
      <LiquidGlassButton
        active={recentOpsOpen}
        aria-label="Toggle recent file operations"
        aria-pressed={recentOpsOpen}
        className={`top-action recent-ops-toggle no-drag${recentOpsOpen ? " active" : ""}`}
        data-compact-popover="recent"
        data-compact-popover-trigger="recent"
        data-titlebar-control="recent-operations"
        onClick={onToggleRecent}
        title="Recent file operations"
        tone="control"
        type="button"
      >
        <History size={16} />
        <span className="top-action-label">Recent</span>
      </LiquidGlassButton>
      <WindowChrome />
    </div>
  );
}
