# Open Questions

## 已确认

1. 首版技术栈采用 Electron + React + Rust Core。
   - Electron 负责 Windows 桌面壳。
   - React UI 可复用于未来 Web/Docker。
   - Rust Core 负责所有性能关键路径。

2. 删除行为默认进回收站。
   - 建议默认删除到回收站，永久删除作为高级操作。

3. UI 方向采用分层亚克力毛玻璃设计（Layered Acrylic / Frosted Glass）。
   - 使用 frameless Electron desktop chrome。
   - 所有界面、菜单和交互使用统一设计语言。
   - 玻璃主要用于 chrome 和控制层，不覆盖媒体网格和预览内容层。

## 必须尽快确认

1. 真实文件移动是否允许跨盘。
   - 跨盘移动不是原子操作，需要 copy/verify/delete 策略和失败恢复。

2. 是否要求支持 NAS。
   - NAS 对 watcher、路径权限、缩略图生成速度影响很大。

3. 是否需要多 root 同时浏览。
   - 例如左侧有多个真实目录根，搜索可以跨 root。

4. 是否需要读取但不写入 EXIF/IPTC/XMP。
   - 当前产品边界是用户标签/评分只写数据库，但仍可读取文件自带元数据。

## 技术风险需要原型验证

1. thumbnail pack 是否首版实现。
   - 如果 MVP 使用 hash 分片文件缓存，简单可靠。
   - 如果首版直接做 pack，长期性能更好，但开发复杂度更高。

2. SQLite 在目标设备上的写入和查询表现。
   - 需要 100 万和 500 万模拟数据压测。

3. 视频缩略图生成吞吐。
   - FFmpeg 对大量视频会非常耗 CPU/IO，需要限速和优先级队列。

4. 高级格式解码稳定性。
   - RAW/HEIC/PSD 依赖复杂，应通过 decoder plugin 隔离。

5. Windows 文件身份追踪。
   - 本地 NTFS 可尝试记录 file id。
   - 跨盘、FAT、exFAT、NAS 上不能完全依赖 file id。

## 当前建议

先做一个高性能浏览原型，不先做完整格式支持。

原型验收顺序：

1. 生成 10 万和 100 万假媒体索引。
2. UI 虚拟网格滚动稳定。
3. 缩略图缓存命中快速显示。
4. 左右切图预取有效。
5. 再接真实扫描和真实解码。
