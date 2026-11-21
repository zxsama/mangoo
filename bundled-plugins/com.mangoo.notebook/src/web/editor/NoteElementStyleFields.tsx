import { useId, useState, type ReactNode } from "react";
import { ColorPicker, DisclosureSummary, LiquidGlassButton, SettingRow, icons } from "@mangoo/web/plugin-host";
import { customDeclarations, elements, styleDeclarations, type ElementStyle, type NoteEditorStyles, type StyleElement } from "./noteFormat.ts";
import "../notebookSettings.css";

interface Props {
  styles: NoteEditorStyles;
  inheritedStyles?: NoteEditorStyles;
  onChange(next: NoteEditorStyles): void;
  onPreview?(next: NoteEditorStyles | null): void;
  zh: boolean;
  presentation?: "compact" | "settings";
}

export function NoteElementStyleFields({ styles, inheritedStyles, onChange, onPreview, zh, presentation = "compact" }: Props) {
  const id = useId();
  const [element, setElement] = useState<StyleElement>("body");
  const [sampleDraft, setSampleDraft] = useState<NoteEditorStyles | null>(null);
  const entry = styles[element] ?? {};
  const base = inheritedStyles?.[element] ?? {};
  const text = (chinese: string, english: string) => zh ? chinese : english;
  const automatic = text("自动", "Auto");
  const faceNames = {
    "sans-serif": text("无衬线", "Sans serif"), serif: text("衬线", "Serif"), monospace: text("等宽", "Monospace")
  };

  function save(nextEntry: ElementStyle) {
    const next = { ...styles };
    if (Object.keys(nextEntry).length) next[element] = nextEntry;
    else delete next[element];
    onChange(next);
  }
  function update<K extends keyof ElementStyle>(key: K, value: ElementStyle[K]) {
    if (entry[key] === value || (entry[key] === undefined && value === "")) return;
    const next = { ...entry };
    if (value === undefined || value === "") delete next[key];
    else next[key] = value;
    save(next);
  }
  function field(key: string, title: string, control: ReactNode, description?: string) {
    if (presentation === "settings") {
      return <SettingRow title={title} description={description} htmlFor={id + key}>{control}</SettingRow>;
    }
    return <label htmlFor={id + key}>{title}{control}</label>;
  }
  function number(key: "fontSize" | "lineHeight" | "width", title: string, min: number, max: number, step: number) {
    return field(key, title, <input key={element + key + (entry[key] ?? "")} id={id + key}
      className="settings-control-input" type="number" min={min} max={max} step={step}
      defaultValue={entry[key] ?? ""} placeholder={String(base[key] ?? automatic)}
      onBlur={event => {
        const input = event.currentTarget;
        if (input.validity.valid) update(key, input.value === "" ? undefined : input.valueAsNumber);
        else input.value = String(entry[key] ?? "");
      }} onKeyDown={event => { if (event.key === "Enter") event.currentTarget.blur(); }} />);
  }
  function color(key: "color" | "background", title: string, fallback: string) {
    return field(key, title, <span className="nb-element-color">
      <small>{entry[key] ?? base[key] ?? automatic}</small>
      <ColorPicker key={element + key} id={id + key} label={title} zh={zh} value={entry[key] ?? base[key] ?? fallback}
        onPreview={value => {
          const next = value === null ? null : { ...styles, [element]: { ...entry, [key]: value } };
          if (presentation === "settings") setSampleDraft(next);
          onPreview?.(next);
        }}
        onChange={value => update(key, value)} />
    </span>);
  }

  const customTitle = text("自定义 CSS 声明", "Custom CSS declarations");
  const cssInput = <textarea id={id + "customCss"} className="settings-control-input" rows={3} spellCheck={false} maxLength={1800}
    aria-label={text("元素 CSS 声明", "Element CSS declarations")} placeholder={base.customCss || "font-family: Georgia; letter-spacing: 0.04em;"}
    value={entry.customCss ?? ""} onChange={event => update("customCss", event.target.value)} />;
  const resetLabel = text("恢复此元素默认样式", "Restore element defaults");
  const resetProps = { disabled: !Object.keys(entry).length, onClick: () => save({}) };
  const sampleStyles = sampleDraft ?? styles;
  return <>
    {field("element", text("元素", "Element"), <select id={id + "element"} className="settings-control-input" value={element}
      onChange={event => setElement(event.target.value as StyleElement)}>
      {(Object.keys(elements) as StyleElement[]).map(key =>
        <option key={key} value={key}>{elements[key][zh ? 0 : 1]}</option>)}
    </select>, text("默认样式用于本机所有笔记库的 Markdown 写作视图。单篇笔记可分别覆盖字体、颜色等属性。",
      "These defaults apply to Markdown live editing in every notebook on this device. Each note can override individual properties, such as typeface or color."))}
    {element === "image" ? number("width", text("宽度 (%)", "Width (%)"), 10, 100, 1) : <>
      {field("fontFamily", text("字体", "Typeface"), <select id={id + "fontFamily"} className="settings-control-input"
        value={entry.fontFamily ?? ""} onChange={event => update("fontFamily", (event.target.value || undefined) as ElementStyle["fontFamily"])}>
        <option value="">{base.fontFamily ? text("默认", "Default") + " · " + faceNames[base.fontFamily] : automatic}</option>
        {(Object.keys(faceNames) as (keyof typeof faceNames)[]).map(face => <option key={face} value={face}>{faceNames[face]}</option>)}
      </select>)}
      {number("fontSize", text("字号 (px)", "Size (px)"), 8, 96, 1)}
      {field("fontWeight", text("字重", "Weight"), <select id={id + "fontWeight"} className="settings-control-input"
        value={entry.fontWeight ?? ""} onChange={event => update("fontWeight", Number(event.target.value) || undefined)}>
        <option value="">{base.fontWeight ? text("默认", "Default") + " · " + base.fontWeight : automatic}</option>
        {[100, 200, 300, 400, 500, 600, 700, 800, 900].map(weight => <option key={weight}>{weight}</option>)}
      </select>)}
      {number("lineHeight", text("行高", "Line height"), 1, 3, 0.1)}
      {color("color", text("文字颜色", "Text color"), "#f4f7f9")}
    </>}
    {color("background", text("背景色", "Background"), "#22262b")}
    {presentation === "settings" ? <>
      <SettingRow title={text("元素预览", "Element preview")}>
        <div className="nb-style-sample nb-writing" ref={node => { if (node) node.style.cssText = styleDeclarations(sampleStyles.body); }}>
          <span className={`nb-el-${element}`} ref={node => { if (node) node.style.cssText = styleDeclarations(sampleStyles[element]); }}>
            {element === "image" ? <icons.Image size={28} aria-label={text("图片", "Image")} /> :
              element === "body" ? text("把今天的想法记下来。", "Write down today’s thoughts.") : elements[element][zh ? 0 : 1]}
          </span>
        </div>
      </SettingRow>
      <SettingRow title={customTitle} htmlFor={id + "customCss"} stacked>{cssInput}</SettingRow>
      <SettingRow title={text("重置当前元素", "Reset selected element")}>
        <LiquidGlassButton {...resetProps}>{resetLabel}</LiquidGlassButton>
      </SettingRow>
    </> : <>
      <details className="nb-custom-style"><DisclosureSummary label={customTitle} />
        {cssInput}
        {entry.customCss && <small>{text("已应用声明", "Applied declarations")} · {customDeclarations(entry.customCss).length}</small>}
      </details>
      <button type="button" className="nb-reset" {...resetProps}><span data-button-content>{resetLabel}</span></button>
    </>}
  </>;
}
