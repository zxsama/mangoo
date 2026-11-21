import { LiquidGlassButton, SettingRow, SettingsSection, SettingsSegmented, SettingsSwitch } from "@mangoo/web/plugin-host";
import { NoteElementStyleFields } from "./editor/NoteElementStyleFields";
import { resetNotebookPreferences, updateNotebookPreferences, useNotebookPreferences } from "./notebookPreferences";

export function NotebookSettings({ zh = true }: { zh?: boolean }) {
  const preferences = useNotebookPreferences();
  const text = (chinese: string, english: string) => zh ? chinese : english;
  const markdown = text("Markdown 默认视图", "Default Markdown view");
  const html = text("HTML 默认视图", "Default HTML view");
  const outline = text("默认展开文档大纲", "Open the outline by default");
  const wrap = text("源码自动换行", "Wrap source lines");
  return <div data-notebook-settings="">
    <SettingsSection id="notebook-editing" title={text("编辑与阅读", "Editing and reading")}>
      <SettingRow title={markdown} description={text("可在每个笔记标签页临时切换视图。", "You can switch views in each note tab.")}>
        <SettingsSegmented label={markdown} value={preferences.markdownMode}
          options={[{ value: "live", label: text("实时写作", "Live editing") }, { value: "source", label: text("源码", "Source") }]}
          onChange={markdownMode => updateNotebookPreferences({ markdownMode })} />
      </SettingRow>
      <SettingRow title={html} description={text("对照视图并排显示源码和预览。", "Split view shows the source and preview side by side.")}>
        <SettingsSegmented label={html} value={preferences.htmlMode}
          options={[{ value: "code", label: text("源码", "Source") }, { value: "split", label: text("对照", "Split") },
            { value: "preview", label: text("预览", "Preview") }]}
          onChange={htmlMode => updateNotebookPreferences({ htmlMode })} />
      </SettingRow>
      <SettingRow title={outline} description={text("打开 Markdown 笔记时显示标题列表。", "Show the heading list when a Markdown note opens.")}>
        <SettingsSwitch label={outline} checked={preferences.outlineOpen}
          onChange={outlineOpen => updateNotebookPreferences({ outlineOpen })} />
      </SettingRow>
      <SettingRow title={wrap} description={text("在 Markdown 和 HTML 源码中，长行随编辑区宽度折行显示。", "Wrap long lines to the editor width in Markdown and HTML source.")}>
        <SettingsSwitch label={wrap} checked={preferences.wrapSource}
          onChange={wrapSource => updateNotebookPreferences({ wrapSource })} />
      </SettingRow>
    </SettingsSection>
    <SettingsSection id="notebook-styles" title={text("Markdown 元素样式", "Markdown element styles")}>
      <NoteElementStyleFields zh={zh} presentation="settings" styles={preferences.elementStyles}
        onChange={elementStyles => updateNotebookPreferences({ elementStyles })} />
    </SettingsSection>
    <SettingsSection id="notebook-defaults" title={text("恢复默认设置", "Restore defaults")}>
      <SettingRow title={text("本机笔记偏好", "Device notebook preferences")}
        description={text("恢复本机的编辑偏好和默认元素样式。单篇笔记保留自己的样式。", "Reset editing preferences and default element styles on this device. Each note keeps its own styles.")}>
        <LiquidGlassButton onClick={resetNotebookPreferences}>{text("恢复笔记默认设置", "Restore notebook defaults")}</LiquidGlassButton>
      </SettingRow>
    </SettingsSection>
  </div>;
}
