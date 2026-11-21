import { useCallback, useId, useLayoutEffect, useRef, useState, type PointerEvent } from "react";
import { hexToHsv, hsvToHex, normalizePickerHex, type HsvColor } from "./colorPickerValues";
import { X } from "./icons";
import { ParameterSlider } from "./ParameterSlider";
import { LiquidGlassButton } from "./liquid-glass";
import { AnchoredOverlay, useOverlayStack, type OverlayRootId } from "./overlays";
import "./colorPicker.css";

interface Props {
  id?: string;
  label: string;
  value: string;
  onChange(value: string): void;
  onPreview?(value: string | null): void;
  disabled?: boolean;
  showValue?: boolean;
  zh?: boolean;
}

// Library identities may predate HEX-only editing and contain an arbitrary CSS color.
function editableHex(color: string): string {
  const hex = normalizePickerHex(color);
  if (hex) return hex;
  const context = document.createElement("canvas").getContext("2d", { willReadFrequently: true });
  if (!context) return "#ffffff";
  context.fillStyle = "#ffffff";
  context.fillStyle = color;
  context.fillRect(0, 0, 1, 1);
  const [r, g, b] = context.getImageData(0, 0, 1, 1).data;
  return "#" + [r, g, b].map(byte => byte.toString(16).padStart(2, "0")).join("");
}

interface PickerSession {
  sequence: number;
  anchor: HTMLButtonElement;
  original: string;
  originalHue: number;
  rootId: OverlayRootId;
  parentId?: string;
}

export function ColorPicker({ id, label, value, onChange, onPreview, disabled = false, showValue = false, zh = true }: Props) {
  const generatedId = useId();
  const popupId = (id ?? generatedId) + "-color-popup";
  const { getContainingLayer } = useOverlayStack();
  const initialFocus = useRef<HTMLInputElement>(null);
  const [session, setSession] = useState<PickerSession | null>(null);
  const [open, setOpen] = useState(false);
  const [hexText, setHexText] = useState("");
  const [hsv, setHsv] = useState<HsvColor>({ hue: 0, saturation: 0, brightness: 100 });
  const [draft, setDraft] = useState("#ffffff");
  const edited = useRef(false);
  const previewCallback = useRef(onPreview);
  previewCallback.current = onPreview;
  const previewSent = useRef(false);
  const clearPreview = useCallback(() => {
    if (!previewSent.current) return;
    previewSent.current = false;
    previewCallback.current?.(null);
  }, []);
  useLayoutEffect(() => clearPreview, [clearPreview]);
  const close = useCallback(() => {
    clearPreview();
    setOpen(false);
  }, [clearPreview]);
  useLayoutEffect(() => { if (disabled) close(); }, [disabled, close]);

  function begin(anchor: HTMLButtonElement) {
    if (open) { close(); return; }
    anchor.focus();
    const parent = getContainingLayer(anchor);
    const hex = editableHex(value);
    edited.current = false;
    setDraft(hex);
    setHexText(hex.toUpperCase());
    const originalHsv = hexToHsv(hex);
    setHsv(originalHsv);
    // A quick reopen can overlap the previous portal's exit animation. The
    // new layer must register in the parent stack before its input takes focus.
    setSession({ sequence: (session?.sequence ?? 0) + 1, anchor, original: value,
      originalHue: originalHsv.hue, rootId: parent?.rootId ?? "select", parentId: parent?.id });
    setOpen(true);
  }

  function preview(hex: string) {
    edited.current = true;
    setDraft(hex);
    if (onPreview) {
      previewSent.current = true;
      onPreview(hex);
    }
  }
  function changeHsv(next: HsvColor) {
    const hex = hsvToHex(next);
    setHsv(next);
    setHexText(hex.toUpperCase());
    preview(hex);
  }
  function moveMarker(event: PointerEvent<HTMLDivElement>) {
    const bounds = event.currentTarget.getBoundingClientRect();
    if (!bounds.width || !bounds.height) return;
    const clamp = (n: number) => Math.max(0, Math.min(100, n));
    changeHsv({ ...hsv, saturation: clamp((event.clientX - bounds.left) / bounds.width * 100),
      brightness: clamp((bounds.bottom - event.clientY) / bounds.height * 100) });
  }
  const validHex = normalizePickerHex(hexText);
  function confirm() {
    if (!open || !validHex) return;
    close();
    if (edited.current) onChange(validHex);
  }
  const text = (chinese: string, english: string) => zh ? chinese : english;
  const hexLabel = text("HEX 颜色", "HEX color");
  const hueLabel = text("色相", "Hue");
  const sampleColor = edited.current ? draft : session?.original ?? value;
  const triggerColor = open ? sampleColor : value;

  return <>
    <button aria-label={label} aria-description={triggerColor} aria-haspopup="dialog" aria-expanded={open} aria-controls={open ? popupId : undefined}
      className="color-picker-trigger" data-interactive-pointer-target="true" id={id} disabled={disabled}
      type="button" title={label} data-show-value={showValue || undefined} onClick={event => begin(event.currentTarget)}>
      <span className="color-picker-chip" style={{ backgroundColor: triggerColor }} />
      {showValue && <span data-button-content className="color-picker-trigger-value">{triggerColor.toUpperCase()}</span>}
    </button>
    {session && <AnchoredOverlay key={session.sequence} anchor={session.anchor} ariaLabel={label} autoFocus className="color-picker-popup"
      fitToViewport id={popupId} initialFocusRef={initialFocus} minWidth={232} onDismiss={close} open={open}
      parentId={session.parentId} placement="bottom-end" preferredWidth={344} role="dialog" rootId={session.rootId}>
      <div className="color-picker-body">
        <header className="color-picker-heading"><strong>{label}</strong>
          <button className="color-picker-close" type="button" aria-label={text("关闭", "Close")} onClick={close}>
            <X size={16} aria-hidden="true" />
          </button>
        </header>
        <div className="color-picker-mixer">
          <div className="color-picker-plane" aria-hidden="true" data-pointer-glow="off"
            style={{ backgroundColor: `hsl(${hsv.hue} 100% 50%)` }}
            onPointerDown={event => {
              if (!event.isPrimary || event.button !== 0) return;
              event.preventDefault();
              event.currentTarget.setPointerCapture(event.pointerId);
              moveMarker(event);
            }}
            onPointerMove={event => { if (event.currentTarget.hasPointerCapture(event.pointerId)) moveMarker(event); }}>
            <span className="color-picker-marker" style={{ left: `clamp(6px, ${hsv.saturation}%, calc(100% - 6px))`, top: `clamp(6px, ${100 - hsv.brightness}%, calc(100% - 6px))`,
              borderColor: hsv.brightness < 55 || hsv.saturation > 60 ? "white" : "#222" }} />
          </div>
          <div className="color-picker-channels">
            {(["saturation", "brightness"] as const).map(key => {
              const channelLabel = key === "saturation" ? text("饱和度", "Saturation") : text("明度", "Value");
              return <label className="color-picker-channel" key={key}>
                <span>{channelLabel}</span>
                <span className="color-picker-number">
                  <input type="number" aria-label={channelLabel} min={0} max={100} step={1} value={Math.round(hsv[key])}
                    onChange={event => {
                      const next = event.currentTarget.valueAsNumber;
                      if (Number.isFinite(next)) changeHsv({ ...hsv, [key]: Math.min(100, Math.max(0, next)) });
                    }} />
                  <span aria-hidden="true">%</span>
                </span>
              </label>;
            })}
          </div>
        </div>
        <label className="color-picker-hue-row">
          <span>{hueLabel}</span>
          <ParameterSlider aria-label={hueLabel} aria-valuetext={Math.round(hsv.hue) + "°"} data-pointer-glow="off" className="color-picker-hue" min={0} max={360}
            value={Math.round(hsv.hue)} resetLabel={Math.round(session.originalHue) + "°"} resetValue={session.originalHue}
            resetHint={text("双击或按 Enter 恢复打开时的色相", "Double-click or press Enter to restore the original hue")}
            onChange={hue => changeHsv({ ...hsv, hue })} />
          <span className="color-picker-degrees" aria-hidden="true">{Math.round(hsv.hue)}°</span>
        </label>
        <div className="color-picker-colors">
          <div className="color-picker-samples">
            {[[text("原色", "Original"), session.original], [text("新色", "New"), sampleColor]].map(([name, color]) =>
              <div key={name}><span>{name}</span><span className="color-picker-sample"><i style={{ backgroundColor: color }} /></span></div>
            )}
          </div>
          <label className="color-picker-hex">HEX<input aria-label={hexLabel} aria-invalid={!validHex || undefined}
            aria-describedby={!validHex ? popupId + "-error" : undefined} ref={initialFocus} spellCheck={false}
            value={hexText} onChange={event => {
              const raw = event.currentTarget.value;
              setHexText(raw);
              const hex = normalizePickerHex(raw);
              if (hex) { setHsv(hexToHsv(hex, hsv.hue)); preview(hex); }
            }} onKeyDown={event => {
              if (event.key === "Enter") { event.preventDefault(); event.stopPropagation(); confirm(); }
            }} /></label>
        </div>
        {!validHex && <small id={popupId + "-error"} className="color-picker-error" role="alert">{text("请输入 3 或 6 位十六进制颜色。", "Enter a 3- or 6-digit HEX color.")}</small>}
        <footer className="color-picker-actions">
          <LiquidGlassButton type="button" onClick={close}>{text("取消", "Cancel")}</LiquidGlassButton>
          <LiquidGlassButton className="color-picker-confirm" type="button" disabled={!validHex} onClick={confirm}>{text("确认", "Confirm")}</LiquidGlassButton>
        </footer>
      </div>
    </AnchoredOverlay>}
  </>;
}
