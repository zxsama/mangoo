export function NotebookPanel({ enabled }: { enabled: boolean }) {
  return (
    <section aria-label="笔记库使用说明">
      <h3>笔记库</h3>
      <p>
        {enabled
          ? "在主窗口左上角的资源库菜单中选择「创建笔记库…」，也可以切换到已有笔记库。"
          : "启用插件并批准资料库数据权限后，可以创建和打开笔记库。"}
      </p>
      <p>Markdown、HTML 和文档附件保存在笔记库中。可在资源库设置中绑定同步账号。</p>
      {enabled && <p>主窗口「设置 → 笔记」可以调整默认视图、源码换行和 Markdown 排版。</p>}
    </section>
  );
}
