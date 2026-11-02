# Plugin And Web Roadmap

## 插件原则

插件系统从首版就预留，但不要让插件进入性能关键路径。

插件必须：

- 有 manifest。
- 声明权限。
- 能被禁用。
- 有版本约束。
- 有独立日志。
- 不能直接任意访问数据库。
- 不能阻塞 UI 和索引主队列。

首版建议只实现插件管理框架和少量内部插件接口，不急于开放第三方插件市场。

## 插件类型

### Decoder Plugin

扩展媒体解码能力。

用途：

- RAW。
- PSD/AI。
- HEIC/AVIF。
- 特殊相机格式。
- 设计软件预览图。

要求：

- 子进程隔离。
- 输入文件路径，输出 metadata 和 thumbnail。
- 有超时和内存限制。

### Metadata Plugin

扩展元数据提取。

用途：

- EXIF。
- IPTC。
- XMP。
- 视频 codec 信息。
- 自定义 sidecar 读取。

首版用户元数据不写 sidecar，但可以读取外部 metadata。

### Action Plugin

扩展右键操作或批处理命令。

用途：

- 用外部编辑器打开。
- 批量转换。
- 批量压缩。
- 调用脚本。

### Import Provider Plugin

未来用于浏览器扩展、截图工具、剪贴板导入。

首版只设计接口，不实现浏览器扩展。

未来浏览器扩展流程：

1. 浏览器扩展选择图片/页面资源。
2. 扩展发送到本机 Mangoo Native Host 或 Mangoo Core。
3. 用户选择真实目录保存。
4. Core 保存文件到指定目录。
5. watcher/indexer 自动入库。

## 插件 manifest 草案

```json
{
  "id": "com.mangoo.decoder.raw",
  "name": "RAW Decoder",
  "version": "0.1.0",
  "engine": "process",
  "entry": "decoder.exe",
  "capabilities": ["decoder"],
  "permissions": ["read-media-file"],
  "formats": [".cr2", ".nef", ".arw", ".dng"]
}
```

## Web/Docker 路线

为了未来部署网页端，首版就要避免把业务逻辑写死在 Electron/Tauri 里。

架构要求：

- Core Service 可以 headless 运行。
- UI 通过 HTTP API 调用 Core。
- React UI 可以被桌面壳加载，也可以被 Core 作为 Web 静态资源提供。
- Windows 文件操作、回收站、系统对话框是 Desktop Adapter。
- Docker/Linux 文件操作是 Server Adapter。

Docker 形态：

```text
docker run
  -v /media/photos:/library/photos:ro
  -v /media/videos:/library/videos:ro
  -v mangoo-data:/data
  -p 8080:8080
  mangoo/server
```

Web 版必须补充：

- 用户认证。
- 路径挂载白名单。
- 只读/可写 root 权限。
- 后台任务面板。
- 网络传输下的缩略图和视频 range streaming。

Windows 桌面版可以默认本机免登录，但 Core API 必须绑定 localhost 或 named pipe，并使用随机 session token。

