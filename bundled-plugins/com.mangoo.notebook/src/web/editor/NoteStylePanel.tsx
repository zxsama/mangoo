import { AnchoredOverlay } from "@mangoo/web/plugin-host";
import { NoteElementStyleFields } from "./NoteElementStyleFields";
import type { NoteEditorStyles } from "./noteFormat";

interface Props {
  anchor: HTMLElement | null;
  onClose(): void;
  zh: boolean;
  styles: NoteEditorStyles;
  inheritedStyles?: NoteEditorStyles;
  onChange(styles: NoteEditorStyles): void;
  onPreview(styles: NoteEditorStyles | null): void;
}

export function NoteStylePanel({ anchor, onClose, zh, styles, inheritedStyles, onChange, onPreview }: Props) {
  const label = zh ? "元素样式" : "Element styles";
  return <AnchoredOverlay anchor={anchor} ariaLabel={label} autoFocus trapFocus
    id="notebook-element-styles" rootId="plugin-view" role="dialog" placement="bottom-end"
    preferredWidth={370} open={Boolean(anchor)} onDismiss={onClose} restoreFocusTarget={anchor}>
    <div className="nb-style-panel">
      <header>
        <h3>{label}</h3>
        <p>{zh ? "此处的更改随当前笔记保存和同步。" : "Changes are saved and synced with this note."}</p>
      </header>
      <NoteElementStyleFields zh={zh} styles={styles} inheritedStyles={inheritedStyles} onChange={onChange} onPreview={onPreview} />
    </div>
  </AnchoredOverlay>;
}
