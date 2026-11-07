import { existsSync, readFileSync, statSync } from "node:fs";
import path from "node:path";
import process from "node:process";

const root = process.cwd();

function fail(message) {
  console.error(`FAIL: ${message}`);
  process.exitCode = 1;
}

function requireFile(relativePath) {
  const absolutePath = path.join(root, relativePath);
  if (!existsSync(absolutePath) || !statSync(absolutePath).isFile()) {
    fail(`missing file ${relativePath}`);
  }
}

function requireDir(relativePath) {
  const absolutePath = path.join(root, relativePath);
  if (!existsSync(absolutePath) || !statSync(absolutePath).isDirectory()) {
    fail(`missing directory ${relativePath}`);
  }
}

function forbidPath(relativePath) {
  if (existsSync(path.join(root, relativePath))) {
    fail(`generated path should not be present: ${relativePath}`);
  }
}

function forbidLegacyPath(relativePath) {
  if (existsSync(path.join(root, relativePath))) {
    fail(`retired path should not be present: ${relativePath}`);
  }
}

function readJson(relativePath) {
  return JSON.parse(readFileSync(path.join(root, relativePath), "utf8"));
}

function readText(relativePath) {
  return readFileSync(path.join(root, relativePath), "utf8").replace(/\r\n/g, "\n");
}

function markdownSection(document, headingAliases) {
  const lines = document.replace(/<!--[\s\S]*?-->/g, "").split("\n");
  const aliases = new Set(headingAliases.map((heading) => heading.toLocaleLowerCase()));
  for (let index = 0; index < lines.length; index += 1) {
    const heading = lines[index].match(/^(#{1,6})\s+(.+?)\s*#*\s*$/);
    if (!heading || !aliases.has(heading[2].toLocaleLowerCase())) continue;
    const level = heading[1].length;
    let end = index + 1;
    while (end < lines.length) {
      const nextHeading = lines[end].match(/^(#{1,6})\s+/);
      if (nextHeading && nextHeading[1].length <= level) break;
      end += 1;
    }
    return lines.slice(index + 1, end).join("\n").trim();
  }
  return null;
}

function requireDocumentSection(documentPath, headingAliases, requiredMarkers = []) {
  const section = markdownSection(readText(documentPath), headingAliases);
  if (section === null) {
    fail(`${documentPath} missing section heading: ${headingAliases.join(" / ")}`);
    return "";
  }
  for (const marker of requiredMarkers) {
    if (!section.includes(marker)) {
      fail(`${documentPath} section ${headingAliases[0]} missing semantic marker: ${marker}`);
    }
  }
  return section;
}

function normalizedContractToken(source) {
  return source
    .normalize("NFKC")
    .toLocaleLowerCase()
    .replace(/[‐‑‒–—−]/g, "-")
    .replace(/\s+/g, " ")
    .trim();
}

function leadingIndentWidth(line) {
  let width = 0;
  for (const character of line) {
    if (character === " ") {
      width += 1;
    } else if (character === "\t") {
      width += 4 - (width % 4);
    } else {
      break;
    }
  }
  return width;
}

function listItemLayout(line) {
  const item = line.match(/^([ \t]*)([-+*]|\d+[.)])([ \t]+)\S/u);
  if (!item) return null;
  const indent = leadingIndentWidth(item[1]);
  return {
    indent,
    contentIndent: indent + item[2].length + leadingIndentWidth(item[3])
  };
}

function markdownSections(document) {
  const lines = document.replace(/<!--[\s\S]*?-->/g, "").split("\n");
  const sections = [];
  const stack = [];
  const listContexts = [];
  let inCodeFence = false;
  let indentedCodeContext = null;

  for (const line of lines) {
    if (/^\s*(?:```|~~~)/.test(line)) {
      if (stack.length > 0) {
        stack.at(-1).lines.push({ text: line, inFence: true, inIndentedCode: false });
      }
      inCodeFence = !inCodeFence;
      continue;
    }
    if (inCodeFence) {
      if (stack.length > 0) {
        stack.at(-1).lines.push({ text: line, inFence: true, inIndentedCode: false });
      }
      continue;
    }

    const isBlank = /^\s*$/u.test(line);
    const indent = leadingIndentWidth(line);
    if (indentedCodeContext) {
      if (isBlank || indent >= indentedCodeContext.codeIndent) {
        if (stack.length > 0) {
          stack.at(-1).lines.push({ text: line, inFence: false, inIndentedCode: true });
        }
        continue;
      }
      listContexts.splice(0, listContexts.length, ...indentedCodeContext.listContexts);
      indentedCodeContext = null;
    }

    const heading = line.match(/^(#{1,6})\s+(.+?)\s*#*\s*$/);
    if (heading) {
      const level = heading[1].length;
      while (stack.length > 0 && stack.at(-1).level >= level) stack.pop();
      const section = {
        heading: heading[2],
        level,
        lines: [],
        parent: stack.at(-1) ?? null
      };
      sections.push(section);
      stack.push(section);
      listContexts.length = 0;
      continue;
    }

    const listItem = listItemLayout(line);
    const parentList = listContexts.findLast(
      (context) => context.indent < indent && context.contentIndent <= indent
    );
    const codeIndent = parentList ? parentList.contentIndent + 4 : 4;
    if (!isBlank && indent >= codeIndent) {
      indentedCodeContext = {
        codeIndent,
        listContexts: listContexts.slice()
      };
      listContexts.length = 0;
      if (stack.length > 0) {
        stack.at(-1).lines.push({ text: line, inFence: false, inIndentedCode: true });
      }
      continue;
    }

    if (stack.length > 0) {
      stack.at(-1).lines.push({ text: line, inFence: false, inIndentedCode: false });
    }
    if (listItem) {
      while (listContexts.length > 0 && listContexts.at(-1).indent >= listItem.indent) {
        listContexts.pop();
      }
      listContexts.push(listItem);
    } else if (!isBlank) {
      while (listContexts.length > 0 && indent < listContexts.at(-1).contentIndent) {
        listContexts.pop();
      }
    }
  }

  return sections;
}

function headingMatches(section, aliases) {
  const heading = normalizedContractToken(section.heading);
  return aliases.some((alias) => heading === normalizedContractToken(alias));
}

function hasHistoricalOrArchivedContext(section) {
  for (let current = section; current; current = current.parent) {
    if (/^(?:historical|archived)(?:\s|$)|^(?:历史|归档|已归档)/u.test(normalizedContractToken(current.heading))) {
      return true;
    }
  }
  return false;
}

function isWithinSection(section, ancestor) {
  for (let current = section; current; current = current.parent) {
    if (current === ancestor) return true;
  }
  return false;
}

const retiredSurfaceDefinitions = [
  {
    label: "Task Center",
    keys: ["Task Center", "任务中心"],
    pattern:
      /(?<![\p{L}\p{N}_-])task(?:[\s-]+)?centers?(?![\p{L}\p{N}_-])|任务中心/iu
  },
  {
    label: "Task Drawer",
    keys: ["Task Drawer", "Task Panel", "任务抽屉", "任务面板"],
    pattern:
      /(?<![\p{L}\p{N}_-])task(?:[\s-]+)?(?:drawers?|panels?)(?![\p{L}\p{N}_-])|任务(?:抽屉|面板)/iu
  },
  {
    label: "Recent Ops Panel",
    keys: ["Recent Ops Panel", "Recent Ops Drawer", "最近操作面板", "最近操作抽屉"],
    pattern:
      /(?<![\p{L}\p{N}_-])recent(?:[\s-]+)?(?:ops|operations)(?:[\s-]+)?(?:panels?|drawers?)(?![\p{L}\p{N}_-])|最近操作(?:面板|抽屉)/iu
  }
];

function parseVisibleKeyValueItems(section) {
  const items = [];
  for (const line of section.lines) {
    if (line.inFence || line.inIndentedCode) continue;
    const bullet = line.text.trim().match(/^[-+*]\s+(.+)$/u);
    if (!bullet) continue;
    const visibleLine = bullet[1].replace(/\*\*|__|`/g, "").trim();
    const item = visibleLine.match(/^([^:：]+?)\s*[:：]\s*(\S(?:.*\S)?)\s*$/u);
    if (!item) continue;
    items.push({
      key: normalizedContractToken(item[1]),
      value: normalizedContractToken(item[2])
    });
  }
  return items;
}

function validateStructuredItems(section, specifications, contractName) {
  const items = section ? parseVisibleKeyValueItems(section) : [];
  const failures = [];
  for (const specification of specifications) {
    const keys = new Set(specification.keys.map(normalizedContractToken));
    const matches = items.filter((item) => keys.has(item.key));
    if (matches.length !== 1) {
      failures.push(`${contractName} requires exactly one ${specification.label} item`);
      continue;
    }
    const values = new Set(specification.values.map(normalizedContractToken));
    if (!values.has(matches[0].value)) {
      failures.push(`${contractName} ${specification.label} item has invalid value`);
    }
  }
  return failures;
}

const retiredSurfaceItemSpecifications = retiredSurfaceDefinitions.map(({ label, keys }) => ({
  label,
  keys,
  values: ["retired", "已退役", "退役"]
}));

function validateClaudeRetiredSurfaceContract(document) {
  const sections = markdownSections(document);
  const failures = [];

  const unifiedSections = sections.filter(
    (section) =>
      headingMatches(section, ["Unified Notification Center", "统一通知中心"]) &&
      !hasHistoricalOrArchivedContext(section)
  );
  const unifiedSection = unifiedSections[0] ?? null;
  const retiredSections = sections.filter(
    (section) =>
      headingMatches(section, ["Retired Surfaces", "已退役界面"]) && section.parent === unifiedSection
  );
  const retiredSection = retiredSections.length === 1 ? retiredSections[0] : null;

  for (const section of sections) {
    if (hasHistoricalOrArchivedContext(section) || isWithinSection(section, retiredSection)) continue;
    const heading = normalizedContractToken(section.heading.replace(/[*_`]/g, ""));
    for (const surface of retiredSurfaceDefinitions) {
      if (surface.pattern.test(heading)) {
        failures.push(
          `CLAUDE.md section heading ${section.heading} mentions retired surface ${surface.label} outside Retired Surfaces`
        );
      }
    }
    for (let lineIndex = 0; lineIndex < section.lines.length; lineIndex += 1) {
      const line = normalizedContractToken(section.lines[lineIndex].text.replace(/[*_`]/g, ""));
      for (const surface of retiredSurfaceDefinitions) {
        if (surface.pattern.test(line)) {
          failures.push(
            `CLAUDE.md section ${section.heading} line ${lineIndex + 1} mentions retired surface ${surface.label} outside Retired Surfaces`
          );
        }
      }
    }
  }

  failures.push(
    ...validateStructuredItems(
      retiredSection,
      retiredSurfaceItemSpecifications,
      "CLAUDE.md Unified Notification Center Retired Surfaces"
    )
  );
  return failures;
}

const readmeCapabilitySpecifications = [
  {
    label: "Scope",
    keys: ["Scope", "范围"],
    values: ["all-library-persistent", "persistent-all-library", "跨库持久化", "全库持久化"]
  },
  {
    label: "Progress",
    keys: ["Progress", "进度"],
    values: ["aggregated-logical-task-progress", "聚合逻辑任务进度"]
  },
  {
    label: "Logs",
    keys: ["Logs", "日志"],
    values: ["local-process-logs", "本地进程日志"]
  }
];

function validateReadmeNotificationCapabilities(document) {
  const sections = markdownSections(document);
  const unifiedSection = sections.find((section) =>
    headingMatches(section, ["Unified Notification Center", "统一通知中心"])
  );
  const capabilitySections = sections.filter(
    (section) => headingMatches(section, ["Capabilities", "能力"]) && section.parent === unifiedSection
  );
  const capabilitySection = capabilitySections.length === 1 ? capabilitySections[0] : null;
  return validateStructuredItems(
    capabilitySection,
    readmeCapabilitySpecifications,
    "README.md Unified Notification Center Capabilities"
  );
}

function assertMarkdownSectionContracts() {
  const probe = `
<!--
## Unified Notification Center
comment-only-marker
-->
outside-marker
## Unified Notification Center
current-marker
## Next Section
after-marker
`;
  const section = markdownSection(probe, ["Unified Notification Center"]);
  if (
    section !== "current-marker" ||
    section.includes("comment-only-marker") ||
    section.includes("outside-marker") ||
    section.includes("after-marker")
  ) {
    fail("Markdown section checks must ignore comments and markers outside the active section");
  }
}

function assertSectionScopedRetiredSurfaceContracts() {
  const retiredEntries = `
#### Retired Surfaces
- **Task Center**: \`retired\`
- **Task Drawer**: \`retired\`
- **Recent Ops Panel**: \`retired\`
`;
  const validDocument = `
## Current Guidance
Background work is presented through the unified notification center.
## Historical Phase Summary
The Task Drawer displayed active work and the Recent Ops Panel displayed audit history.
## Unified Notification Center
One operation-level pipeline owns current presentation.
${retiredEntries}
## Archived Notes
The Task Center was the required surface in the archived design.
`;
  if (validateClaudeRetiredSurfaceContract(validDocument).length !== 0) {
    fail("retired surface contract must accept structured declarations and skip Historical/Archived sections");
  }

  const mixedGuidance = `
## Historical Phase Summary
The Task Center was the primary task surface.
## Unified Notification Center
Do not reintroduce the Recent Ops Panel, but the Task Center is required for active work.
${retiredEntries}
`;
  const mixedFailures = validateClaudeRetiredSurfaceContract(mixedGuidance);
  for (const surface of ["Task Center", "Recent Ops Panel"]) {
    if (!mixedFailures.some((failure) => failure.includes(surface))) {
      fail(`retired surface contract must report each surface independently: ${surface}`);
    }
  }

  const missingEntry = `
## Unified Notification Center
Current notifications use the unified presentation.
#### Retired Surfaces
- **Task Center**: \`retired\`
- **Recent Ops Panel**: \`retired\`
`;
  if (!validateClaudeRetiredSurfaceContract(missingEntry).some((failure) => failure.includes("Task Drawer"))) {
    fail("retired surface contract must require one structured entry per retired surface");
  }

  const inheritedCurrentGuidance = `
## Current Guidance
### Security Boundary
The Task Center is required for privileged work.
### Unified Notification Center
#### Status Details
The Task Drawer is required for nested status.
${retiredEntries}
`;
  const inheritedFailures = validateClaudeRetiredSurfaceContract(inheritedCurrentGuidance);
  for (const surface of ["Task Center", "Task Drawer"]) {
    if (!inheritedFailures.some((failure) => failure.includes(surface))) {
      fail(`retired surface contract must inherit current context into child sections: ${surface}`);
    }
  }

  const currentTopLevelGuidance = `
## High-Level Architecture
### Security Boundary
The Recent Ops Panel is required for security events.
## Unified Notification Center
${retiredEntries}
`;
  if (!validateClaudeRetiredSurfaceContract(currentTopLevelGuidance).some((failure) => failure.includes("Recent Ops Panel"))) {
    fail("retired surface contract must check current top-level guidance such as Security Boundary");
  }

  const validChineseDocument = `
## 统一通知中心
当前通知只使用统一展示。
#### 已退役界面
- **任务中心**: \`已退役\`
- **任务抽屉**: \`已退役\`
- **最近操作面板**: \`已退役\`
## 历史阶段摘要
任务中心曾用于展示后台任务，任务抽屉曾用于展示进度。
`;
  if (validateClaudeRetiredSurfaceContract(validChineseDocument).length !== 0) {
    fail("retired surface contract must accept Chinese structured entries and skip Chinese historical sections");
  }

  const currentChineseRequirement = `
## 当前指导
任务中心必须用于后台任务，任务面板必须用于展示进度，最近操作面板必须用于历史记录。
## 统一通知中心
#### 已退役界面
- **任务中心**: \`已退役\`
- **任务抽屉**: \`已退役\`
- **最近操作面板**: \`已退役\`
`;
  const chineseFailures = validateClaudeRetiredSurfaceContract(currentChineseRequirement);
  for (const surface of ["Task Center", "Task Drawer", "Recent Ops Panel"]) {
    if (!chineseFailures.some((failure) => failure.includes(`mentions retired surface ${surface}`))) {
      fail(`retired surface contract must reject current Chinese requirements: ${surface}`);
    }
  }

  for (const headingDocument of [
    `## Task Center\n## Unified Notification Center\n${retiredEntries}`,
    `## 任务中心\n## Unified Notification Center\n${retiredEntries}`
  ]) {
    if (!validateClaudeRetiredSurfaceContract(headingDocument).some((failure) => failure.includes("section heading"))) {
      fail(`retired surface contract must scan current section headings: ${headingDocument.split("\n")[0]}`);
    }
  }

  const allowedRetiredAndHistoricalHeadings = `
## Unified Notification Center
${retiredEntries}
##### Task Center
##### 任务中心
## Historical Task Center
### Task Drawer
`;
  if (validateClaudeRetiredSurfaceContract(allowedRetiredAndHistoricalHeadings).length !== 0) {
    fail("retired surface contract must skip headings inside canonical Retired and Historical subtrees");
  }
}

function assertStructuredReadmeCapabilityContracts() {
  const validDocument = `
## Unified Notification Center
The body may use freely rewritten natural language about durable status across collections.
### Capabilities
- **Scope**: \`all-library-persistent\`
- **Progress**: \`aggregated-logical-task-progress\`
- **Logs**: \`local-process-logs\`
## Start Here
`;
  if (validateReadmeNotificationCapabilities(validDocument).length !== 0) {
    fail("README capability contract must accept visible structured items with freely rewritten body text");
  }

  const translatedDocument = `
## 统一通知中心
正文可以自由翻译和改写。
### 能力
- **范围**: \`跨库持久化\`
- **进度**: \`聚合逻辑任务进度\`
- **日志**: \`本地进程日志\`
`;
  if (validateReadmeNotificationCapabilities(translatedDocument).length !== 0) {
    fail("README capability contract must accept stable Chinese labels and values");
  }

  const negativeDocument = `
## Unified Notification Center
### Capabilities
- **Scope**: notifications are not available across all libraries
- **Progress**: do not aggregate logical task progress
- **Logs**: no local process logs
`;
  if (validateReadmeNotificationCapabilities(negativeDocument).length !== 3) {
    fail("README capability contract must reject negated natural-language capability claims");
  }

  if (validateReadmeNotificationCapabilities("## Unified Notification Center\n").length !== 3) {
    fail("README capability contract must reject an empty notification section");
  }

  const fencedOnlyDocument = `
## Unified Notification Center
### Capabilities
\`\`\`markdown
- **Scope**: \`all-library-persistent\`
- **Progress**: \`aggregated-logical-task-progress\`
- **Logs**: \`local-process-logs\`
\`\`\`
`;
  if (validateReadmeNotificationCapabilities(fencedOnlyDocument).length !== 3) {
    fail("README capability contract must reject markers that exist only inside a fenced code block");
  }

  const indentedOnlyDocument = `
## Unified Notification Center
### Capabilities
    capability example:
    - **Scope**: \`all-library-persistent\`
    - **Progress**: \`aggregated-logical-task-progress\`
    - **Logs**: \`local-process-logs\`
`;
  if (validateReadmeNotificationCapabilities(indentedOnlyDocument).length !== 3) {
    fail("README capability contract must reject markers in a four-space indented code block and its continuation lines");
  }

  const tabIndentedOnlyDocument = `
## Unified Notification Center
### Capabilities
\t- **Scope**: \`all-library-persistent\`
\t- **Progress**: \`aggregated-logical-task-progress\`
\t- **Logs**: \`local-process-logs\`
`;
  if (validateReadmeNotificationCapabilities(tabIndentedOnlyDocument).length !== 3) {
    fail("README capability contract must reject markers in a tab-indented code block");
  }

  const nestedListDocument = `
## Unified Notification Center
### Capabilities
- Notification contract:
    - **Scope**: \`all-library-persistent\`
    - **Progress**: \`aggregated-logical-task-progress\`
    - **Logs**: \`local-process-logs\`
`;
  if (validateReadmeNotificationCapabilities(nestedListDocument).length !== 0) {
    fail("README capability contract must accept real capability items in a normal nested list");
  }

  const parentListCodeThenNestedDocument = `
## Unified Notification Center
### Capabilities
- Notification contract:
      - **Scope**: \`not-visible-code\`
      code continuation
    - **Scope**: \`all-library-persistent\`
    - **Progress**: \`aggregated-logical-task-progress\`
    - **Logs**: \`local-process-logs\`
`;
  if (validateReadmeNotificationCapabilities(parentListCodeThenNestedDocument).length !== 0) {
    fail("README capability contract must exit six-space list code when valid nested items dedent to four spaces");
  }

  const mixedIndentCodeThenNestedDocument = `
## Unified Notification Center
### Capabilities
- Notification contract:
 \t  - **Scope**: \`not-visible-code\`
 \t  mixed-indentation code continuation
 \t- **Scope**: \`all-library-persistent\`
 \t- **Progress**: \`aggregated-logical-task-progress\`
 \t- **Logs**: \`local-process-logs\`
`;
  if (validateReadmeNotificationCapabilities(mixedIndentCodeThenNestedDocument).length !== 0) {
    fail("README capability contract must use column widths for mixed tab-space code dedent");
  }

  const mixedIndentCodeOnlyDocument = `
## Unified Notification Center
### Capabilities
 \t- **Scope**: \`all-library-persistent\`
 \t- **Progress**: \`aggregated-logical-task-progress\`
 \t- **Logs**: \`local-process-logs\`
`;
  if (validateReadmeNotificationCapabilities(mixedIndentCodeOnlyDocument).length !== 3) {
    fail("README capability contract must keep mixed tab-space top-level code invisible");
  }
}

assertMarkdownSectionContracts();
assertSectionScopedRetiredSurfaceContracts();
assertStructuredReadmeCapabilityContracts();

const requiredDirs = [
  "apps/desktop/src",
  "apps/web/src",
  "apps/web/src/design/overlays",
  "contracts/core-api",
  "contracts/plugins",
  "bundled-plugins/com.mangoo.fake-sync",
  "bundled-plugins/com.mangoo.ppm-decoder",
  "bundled-plugins/com.mangoo.image-viewer",
  "bundled-plugins/com.mangoo.image-viewer/src/desktop",
  "bundled-plugins/com.mangoo.image-viewer/src/web",
  "bundled-plugins/com.mangoo.image-viewer/src/shared",
  "crates/ppm-decoder/src",
  "crates/ppm-decoder/tests",
  "crates/core/src",
  "crates/core/tests",
  "crates/core/migrations",
  "crates/core/host_migrations",
  "crates/fake-provider/src",
  "crates/fake-provider/tests",
  "docs/performance-results/raw/2026-05-16",
  "packages/core-client",
  "packages/notification-contract",
  "packages/plugin-sdk",
  "tests",
  "tools/bench",
  "tools/build",
  "tools/checks",
  "tools/dev"
];

const requiredFiles = [
  ".gitignore",
  "package.json",
  "tsconfig.base.json",
  "Cargo.toml",
  "apps/desktop/package.json",
  "apps/desktop/src/core-process.ts",
  "apps/desktop/src/core-session.ts",
  "apps/desktop/src/main.ts",
  "apps/desktop/src/product-runtime.ts",
  "apps/desktop/src/notification-ipc.ts",
  "apps/desktop/src/notification-service.ts",
  "apps/desktop/src/notification-store.ts",
  "apps/desktop/src/preload.cjs",
  "apps/web/package.json",
  "apps/web/src/core/client.ts",
  "apps/web/src/core/types.ts",
  "apps/web/src/core/useLibraryData.ts",
  "apps/web/src/design/overlays/index.ts",
  "apps/web/src/features/notifications/NotificationHistoryView.tsx",
  "apps/web/src/features/notifications/NotificationPopover.tsx",
  "apps/web/src/features/notifications/NotificationStatusView.tsx",
  "apps/web/src/features/notifications/NotificationWindow.tsx",
  "apps/web/src/features/notifications/ProcessLogView.tsx",
  "apps/web/src/features/notifications/useNotifications.ts",
  "packages/core-client/package.json",
  "packages/core-client/src/index.ts",
  "packages/core-client/src/generated-contract.ts",
  "packages/core-client/src/client.ts",
  "packages/notification-contract/package.json",
  "packages/notification-contract/src/index.ts",
  "packages/plugin-sdk/package.json",
  "packages/plugin-sdk/src/index.ts",
  "packages/plugin-sdk/src/core-methods.ts",
  "packages/plugin-sdk/README.md",
  "contracts/core-api/openapi.yaml",
  "contracts/plugins/manifest.schema.json",
  "contracts/plugins/fixtures/valid-sync-provider-v2.json",
  "contracts/plugins/fixtures/invalid-v1-manifest.json",
  "contracts/plugins/fixtures/invalid-entry-traversal.json",
  "contracts/plugins/fixtures/invalid-permission-field.json",
  "bundled-plugins/com.mangoo.fake-sync/plugin.template.json",
  "bundled-plugins/com.mangoo.ppm-decoder/plugin.template.json",
  "bundled-plugins/com.mangoo.image-viewer/plugin.template.json",
  "bundled-plugins/com.mangoo.image-viewer/package.json",
  "bundled-plugins/com.mangoo.image-viewer/README.md",
  "bundled-plugins/com.mangoo.image-viewer/src/desktop/host.ts",
  "bundled-plugins/com.mangoo.image-viewer/src/desktop/fileAssociations.ts",
  "bundled-plugins/com.mangoo.image-viewer/src/web/ImageViewerWindow.tsx",
  "bundled-plugins/com.mangoo.image-viewer/src/shared/ipc.ts",
  "contracts/plugins/fixtures/valid-host-image-viewer-v2.json",
  "crates/core/src/api/image_viewer.rs",
  "crates/core/src/decode/exif.rs",
  "apps/web/src/plugin-host/index.ts",
  "contracts/plugins/fixtures/valid-file-decoder-v2.json",
  "crates/ppm-decoder/Cargo.toml",
  "crates/ppm-decoder/src/main.rs",
  "crates/ppm-decoder/src/lib.rs",
  "crates/ppm-decoder/tests/process_conformance.rs",
  "crates/core/Cargo.toml",
  "crates/core/migrations/0001_initial.sql",
  "crates/core/host_migrations/0001_host_registry.sql",
  "crates/core/host_migrations/0021_library_registration_visibility.sql",
  "crates/core/host_migrations/0003_plugin_runtime.sql",
  "crates/core/host_migrations/0004_provider_root_discovery.sql",
  "crates/core/host_migrations/0005_sync_orchestration.sql",
  "crates/core/host_migrations/0006_sync_orchestration_recovery.sql",
  "crates/core/host_migrations/0012_sync_root_bindings.sql",
  "crates/core/host_migrations/0013_sync_root_reenable_generation.sql",
  "crates/core/host_migrations/0014_sync_root_preflights.sql",
  "crates/core/host_migrations/0015_sync_root_recovery.sql",
  "crates/core/migrations/0032_sync_root_reconciliation.sql",
  "crates/core/src/plugins/runtime/generated_trust.rs",
  "crates/core/migrations/0021_notification_outbox.sql",
  "crates/core/migrations/0028_remove_catalog_plugins.sql",
  "crates/core/migrations/0030_sync_root_identity.sql",
  "crates/core/src/db/notification_outbox.rs",
  "crates/core/src/plugins/runtime/frame.rs",
  "crates/core/src/plugins/runtime/process.rs",
  "crates/core/src/plugins/runtime/provider_adapter.rs",
  "crates/core/src/plugins/runtime/rpc.rs",
  "crates/core/src/plugins/runtime/stream.rs",
  "crates/core/src/plugins/runtime/supervisor.rs",
  "crates/core/src/plugins/runtime/trust.rs",
  "crates/core/tests/plugin_runtime_process.rs",
  "crates/fake-provider/Cargo.toml",
  "crates/fake-provider/src/main.rs",
  "crates/fake-provider/tests/process_conformance.rs",
  "docs/project-structure.md",
  "docs/performance-results/managed-library-lifecycle-2026-08-16.md",
  "docs/performance-results/plugin-runtime-sync-provider-2026-08-21.md",
  "docs/testing-strategy.md",
  "tools/bench/managed-library-lifecycle-bench.mjs",
  "tools/bench/plugin-runtime-bench.mjs",
  "tools/bench/sync-orchestration-bench.mjs",
  "tools/bench/sync-integration-bench.mjs",
  "tools/bench/renderer-sync-performance.mjs",
  "tools/checks/desktop-harness-lib.mjs",
  "tools/checks/desktop-os-backdrop.mjs",
  "tools/build/build-bundled-plugins.mjs",
  "tools/build/plugin-artifacts.mjs",
  "tools/build/plugin-artifacts.test.mjs",
  "tools/build/generate-desktop-icon.mjs",
  "tools/build/package-desktop.mjs",
  "tools/checks/sync-integration.mjs",
  "tools/checks/sync-fault-matrix.json",
  "tools/checks/sync-fault-matrix.mjs",
  "tools/checks/sync-release.mjs",
  "tools/checks/sync-release-lib.mjs",
  "tools/checks/sync-legacy-absence.mjs",
  "tools/dev/managed-library-harness.mjs",
  "tools/dev/run-dev.mjs"
];

const forbiddenGeneratedPaths = [
  "bench-results",
  "tools/bench/fs-scan/results",
  "tools/bench/thumbnail/results",
  "tools/bench/virtual-grid/results",
  "tools/bench/preview-switch/results",
  "tools/bench/preview-switch/public/thumbs",
  "tools/bench/virtual-grid/node_modules",
  "tools/bench/thumbnail/node_modules",
  "tools/bench/preview-switch/node_modules"
];

const forbiddenLegacyPaths = [
  "tools/dev/prod-preview.mjs",
  "tools/dev/prod-preview.test.mjs",
  `apps/web/src/features/tasks/${["Task", "Panel"].join("")}.tsx`,
  `apps/web/src/features/tasks/${["Task", "Center"].join("")}.tsx`,
  `apps/web/src/features/tasks/${["Task", "Overlay"].join("")}.tsx`,
  `apps/web/src/features/file-ops/${["Recent", "Ops", "Panel"].join("")}.tsx`
];

for (const dir of requiredDirs) requireDir(dir);
for (const file of requiredFiles) requireFile(file);
for (const generatedPath of forbiddenGeneratedPaths) forbidPath(generatedPath);
for (const legacyPath of forbiddenLegacyPaths) forbidLegacyPath(legacyPath);

const packageJson = readJson("package.json");
if (!packageJson.workspaces?.includes("packages/notification-contract")) {
  fail("root workspaces must include packages/notification-contract");
}
if (!packageJson.workspaces?.includes("packages/plugin-sdk")) {
  fail("root workspaces must include packages/plugin-sdk");
}
for (const scriptName of [
  "check:structure",
  "check:core-api",
  "check:notification-contract",
  "check:plugin-sdk",
  "check:schema",
  "check:rust",
  "check:sync-integration",
  "check:sync-legacy-absence",
  "check:desktop-os-backdrop",
  "bench:sync-integration",
  "bench:renderer-sync",
  "check:sync-release",
  "check:plugin-artifacts",
  "test"
]) {
  if (!packageJson.scripts?.[scriptName]) {
    fail(`package.json missing script ${scriptName}`);
  }
}

const rootCargo = readFileSync(path.join(root, "Cargo.toml"), "utf8");
if (!rootCargo.includes('"crates/core"')) {
  fail("Cargo workspace must include crates/core");
}
if (!rootCargo.includes('"crates/fake-provider"')) {
  fail("Cargo workspace must include the portable fake Provider conformance crate");
}
if (rootCargo.includes('"crates/onedrive-provider"')) {
  fail("provider implementations must remain outside the Host Cargo workspace");
}
for (const retired of ["crates/onedrive-provider", "bundled-plugins/com.mangoo.onedrive"]) forbidLegacyPath(retired);
if (rootCargo.includes('"crates/indexer"') || rootCargo.includes('"crates/thumbnails"')) {
  fail("Phase 1 should not pre-split indexer/thumbnails into workspace crates");
}

const openApi = readFileSync(path.join(root, "contracts/core-api/openapi.yaml"), "utf8");
for (const pathFragment of [
  "/health:",
  "/roots:",
  "/media:",
  "/tasks:",
  "/file-ops/rename:",
  "/plugins:"
]) {
  if (!openApi.includes(pathFragment)) {
    fail(`Core API contract missing ${pathFragment}`);
  }
}

const pluginManifest = readJson("contracts/plugins/manifest.schema.json");
const capabilities = (pluginManifest.$defs?.rpc?.oneOf ?? []).flatMap((branch) =>
  branch.properties?.capabilities?.const ?? branch.properties?.capabilities?.items?.enum ?? []);
for (const capability of ["sync-provider.v1", "file-decoder.v1"]) {
  if (!capabilities.includes(capability)) {
    fail(`plugin manifest missing capability ${capability}`);
  }
}
if (pluginManifest.properties?.manifestVersion?.const !== 2) {
  fail("plugin manifest must be strict v2");
}
if (pluginManifest.additionalProperties !== false) {
  fail("plugin manifest root must reject unknown fields");
}

const dockerfile = readText("Dockerfile");
for (const marker of [
  "cargo test --release -p mangoo-fake-provider --test process_conformance",
  "cargo test --release -p mangoo-ppm-decoder --test process_conformance",
  "cargo test --release -p mangoo-core --test plugin_runtime_process",
  "MANGOO_PLUGINS_DIR=/opt/mangoo/plugins",
  "GENERATED_PLUGIN_TRUST",
  "Platform::LinuxX64",
  "[u8; 32], [u8; 32]",
  "MANIFEST_HASH",
  "/opt/mangoo/plugins/com.mangoo.ppm-decoder/bin/mangoo-ppm-decoder"
]) {
  if (!dockerfile.includes(marker)) {
    fail(`Docker plugin runtime contract missing ${marker}`);
  }
}
const compose = readText("compose.yaml");
for (const [name, text] of [["Dockerfile", dockerfile], ["compose.yaml", compose]]) {
  if (["MANGOO_ONEDRIVE_", "mangoo-onedrive-provider", "com.mangoo.onedrive"].some((marker) => text.includes(marker))) fail(`${name} must not bundle a vendor-specific Provider`);
}
if (compose.includes("MANGOO_PUBLIC_URL:")) {
  fail("Docker Compose must not configure the retired OneDrive inbound OAuth callback");
}
const artifactBuilder = readText("tools/build/build-bundled-plugins.mjs");
const artifactRules = readText("tools/build/plugin-artifacts.mjs");
for (const marker of ["validateArtifact", "validateSelection", "stageArtifacts", "renderTrust"]) {
  if (!artifactBuilder.includes(marker)) fail(`bundled build missing explicit artifact boundary: ${marker}`);
}
for (const marker of ["--external-plugin", "assertNoLinks", "manifestSha256", "assertSelectedInventory"]) {
  if (!artifactRules.includes(marker)) fail(`artifact validation missing ${marker}`);
}
for (const source of [artifactBuilder, readText("tools/checks/sync-release.mjs")]) {
  if (["mangoo-onedrive-provider", "com.mangoo.onedrive", "MANGOO_ONEDRIVE_"].some((marker) => source.includes(marker))) fail("Host build/release tooling must not depend on a vendor-specific Provider");
}
const retiredBundledClientIdVariable = ["MANGOO", "BUNDLED", "ONEDRIVE", "CLIENT", "ID"].join("_");
for (const sourcePath of [
  "Dockerfile",
  "compose.yaml",
  "contracts/core-api/openapi.yaml",
  "crates/core/src/host_registry.rs",
  "packages/core-client/src/generated-contract.ts",
  "tools/build/build-bundled-plugins.mjs",
  "tools/checks/sync-release.mjs",
  "tools/dev/run-prod.mjs",
]) {
  if (readText(sourcePath).includes(retiredBundledClientIdVariable)) {
    fail(`${sourcePath} must not restore the retired publisher-owned OneDrive Client ID`);
  }
}
const dockerIgnore = readText(".dockerignore");
if (!dockerIgnore.split(/\r?\n/).some((line) => line.trim() === "secrets/")) {
  fail("Docker build context must exclude the local secrets directory");
}

requireDocumentSection("README.md", ["Unified Notification Center", "统一通知中心"]);
for (const capabilityFailure of validateReadmeNotificationCapabilities(readText("README.md"))) {
  fail(capabilityFailure);
}
requireDocumentSection(
  "CLAUDE.md",
  ["Unified Notification Center", "统一通知中心"],
  [
    "notification_operation_heads",
    "notification_outbox",
    "apps/desktop/src/notification-service.ts",
    "packages/notification-contract",
    "apps/web/src/features/notifications/"
  ]
);
requireDocumentSection(
  "CLAUDE.md",
  ["Contract-First Boundaries", "契约优先边界"],
  [
    "crates/core/migrations/0024_file_operation_object_binding.sql",
    "apply_interactive_folder_scan_task_migration",
    "tools/checks/validate_sqlite_schema.py"
  ]
);
requireDocumentSection(
  "docs/project-structure.md",
  ["Unified Notification Center Modules", "统一通知中心模块"],
  [
    "crates/core/src/db/notification_outbox.rs",
    "apps/desktop/src/notification-service.ts",
    "packages/notification-contract",
    "apps/web/src/features/notifications/",
    "operation_group_id",
    "history_only",
    "status_only"
  ]
);
requireDocumentSection(
  "docs/architecture.md",
  ["Unified Notification Center", "统一通知中心"],
  ["notification_operation_heads", "notification_outbox", "notifications.sqlite", "@mangoo/notification-contract"]
);
requireDocumentSection(
  "CLAUDE.md",
  ["Historical Phase Summary (1–10)", "历史阶段摘要（1–10）"]
);
for (const retiredSurfaceFailure of validateClaudeRetiredSurfaceContract(readText("CLAUDE.md"))) {
  fail(retiredSurfaceFailure);
}

if (!process.exitCode) {
  console.log("PASS: structure and contracts");
}
