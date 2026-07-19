// Static confirmation copy for hard-removing a folder (root) from the library.
// Names ONLY the UNRECOVERABLE data (ratings/tags/notes); index + thumbnails are
// rebuildable by re-adding, and disk files are never touched.
export const REMOVE_ROOT_CONFIRM = {
  title: (name: string) => `从库移除「${name}」?`,
  body:
    "该文件夹的索引和缩略图将被删除(重新添加可重建)。\n" +
    "⚠️ 评分、标签、备注将永久丢失,无法恢复。\n" +
    "磁盘上的原文件不受影响。",
  confirmLabel: "移除",
  cancelLabel: "取消"
} as const;
