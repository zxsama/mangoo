const REQUIRED_PROPERTIES = [
  'backgroundMaterial: "acrylic"',
  "transparent: false",
  'backgroundColor: "#00000000"',
  "roundedCorners: true",
  "frame: false"
];
const NO_TOP_LEVEL_SPREAD = "no top-level spread in BrowserWindow options";

export function inspectNativeBrowserWindowOptions(source) {
  const optionsObjects = extractBrowserWindowOptionsObjects(source);
  if (optionsObjects.length === 0) {
    return {
      browserWindowOptionsFound: false,
      browserWindowCount: 0,
      windows: [],
      backgroundMaterial: null,
      backgroundMaterialSource: null,
      frameFalse: false,
      transparent: false,
      nonLayeredHost: false,
      transparentBackgroundColor: false,
      roundedCorners: false,
      disablesNativeMaterial: false,
      unsafeTopLevelSpreads: [],
      missingRequiredProperties: [...REQUIRED_PROPERTIES]
    };
  }

  const windows = optionsObjects.map((optionsObject, index) =>
    inspectBrowserWindowOptionsObject(optionsObject, index)
  );
  const firstNonCompliant = windows.find((window) => window.missingRequiredProperties.length) ?? windows[0];
  const unsafeTopLevelSpreads = windows.flatMap((window) => window.unsafeTopLevelSpreads);
  const missingRequiredProperties = uniqueInOrder(
    windows.flatMap((window) => window.missingRequiredProperties)
  );

  return {
    browserWindowOptionsFound: true,
    browserWindowCount: windows.length,
    windows,
    backgroundMaterial: firstNonCompliant.backgroundMaterial,
    backgroundMaterialSource: firstNonCompliant.backgroundMaterialSource,
    frameFalse: windows.every((window) => window.frameFalse),
    transparent: windows.every((window) => window.transparent),
    nonLayeredHost: windows.every((window) => window.nonLayeredHost),
    transparentBackgroundColor: windows.every((window) => window.transparentBackgroundColor),
    roundedCorners: windows.every((window) => window.roundedCorners),
    disablesNativeMaterial: windows.some((window) => window.disablesNativeMaterial),
    unsafeTopLevelSpreads,
    missingRequiredProperties
  };
}

function inspectBrowserWindowOptionsObject(optionsObject, windowIndex) {
  const { properties, unsafeTopLevelSpreads } = extractTopLevelObjectEntries(
    optionsObject,
    windowIndex
  );
  const latest = latestPropertiesByName(properties);
  const backgroundMaterialProperties = properties.filter(
    (property) => property.name === "backgroundMaterial"
  );
  const backgroundMaterialProperty = latest.get("backgroundMaterial") ?? null;
  const backgroundMaterial = backgroundMaterialProperty
    ? stringLiteralValue(backgroundMaterialProperty.value)
    : null;
  const frameFalse = booleanLiteralValue(latest.get("frame")?.value) === false;
  const transparent = booleanLiteralValue(latest.get("transparent")?.value) === true;
  const nonLayeredHost = booleanLiteralValue(latest.get("transparent")?.value) === false;
  const transparentBackgroundColor =
    stringLiteralValue(latest.get("backgroundColor")?.value) === "#00000000";
  const roundedCorners = booleanLiteralValue(latest.get("roundedCorners")?.value) === true;
  const disablesNativeMaterial = backgroundMaterialProperties.some(
    (property) => stringLiteralValue(property.value) === "none"
  );

  const missingRequiredProperties = [];
  if (backgroundMaterial !== "acrylic") missingRequiredProperties.push(REQUIRED_PROPERTIES[0]);
  if (!nonLayeredHost) missingRequiredProperties.push(REQUIRED_PROPERTIES[1]);
  if (!transparentBackgroundColor) missingRequiredProperties.push(REQUIRED_PROPERTIES[2]);
  if (!roundedCorners) missingRequiredProperties.push(REQUIRED_PROPERTIES[3]);
  if (!frameFalse) missingRequiredProperties.push(REQUIRED_PROPERTIES[4]);
  if (unsafeTopLevelSpreads.length) missingRequiredProperties.push(NO_TOP_LEVEL_SPREAD);

  return {
    index: windowIndex,
    backgroundMaterial,
    backgroundMaterialSource: backgroundMaterialProperty
      ? normalizedPropertySource(backgroundMaterialProperty)
      : null,
    frameFalse,
    transparent,
    nonLayeredHost,
    transparentBackgroundColor,
    roundedCorners,
    disablesNativeMaterial,
    unsafeTopLevelSpreads,
    missingRequiredProperties
  };
}

function extractBrowserWindowOptionsObjects(source) {
  const masked = maskCommentsAndStrings(source);
  const pattern = /\bnew\s+BrowserWindow\s*\(/g;
  let match;
  const optionsObjects = [];

  while ((match = pattern.exec(masked)) !== null) {
    const openParen = masked.indexOf("(", match.index);
    if (openParen === -1) continue;

    let objectStart = openParen + 1;
    while (/\s/.test(masked[objectStart] ?? "")) objectStart += 1;
    if (source[objectStart] !== "{") continue;

    const objectEnd = findMatchingDelimiterIndex(source, objectStart, "{", "}");
    if (objectEnd === -1) continue;
    optionsObjects.push(source.slice(objectStart, objectEnd + 1));
  }

  return optionsObjects;
}

function latestPropertiesByName(properties) {
  const latest = new Map();
  for (const property of properties) {
    latest.set(property.name, property);
  }
  return latest;
}

function extractTopLevelObjectEntries(objectSource, windowIndex) {
  const body = objectSource.slice(1, -1);
  const unsafeTopLevelSpreads = [];
  const properties = splitTopLevelEntries(body)
    .map((entry) => {
      const trimmedEntry = stripComments(entry).trim();
      if (trimmedEntry.startsWith("...")) {
        // This literal conditional can only add an icon, never a material or
        // transparency option. Undefined icons must be omitted for Electron.
        if (/^\.\.\.\(\s*windowIcon\s*\?\s*\{\s*icon\s*:\s*windowIcon\s*\}\s*:\s*\{\s*\}\s*\)$/.test(trimmedEntry)) {
          return null;
        }
        unsafeTopLevelSpreads.push({
          windowIndex,
          source: trimmedEntry
        });
        return null;
      }

      const colonIndex = findTopLevelColon(entry);
      if (colonIndex === -1) return null;

      const key = objectPropertyKey(entry.slice(0, colonIndex));
      if (!key) return null;

      return {
        name: key,
        key: entry.slice(0, colonIndex).trim(),
        value: entry.slice(colonIndex + 1).trim()
      };
    })
    .filter(Boolean);

  return { properties, unsafeTopLevelSpreads };
}

function uniqueInOrder(values) {
  return [...new Set(values)];
}

function splitTopLevelEntries(source) {
  const entries = [];
  let start = 0;
  let depth = 0;

  for (let index = 0; index < source.length; index += 1) {
    const skipped = skipIgnored(source, index);
    if (skipped !== index) {
      index = skipped - 1;
      continue;
    }

    const char = source[index];
    if (char === "{" || char === "[" || char === "(") depth += 1;
    else if (char === "}" || char === "]" || char === ")") depth = Math.max(0, depth - 1);
    else if (char === "," && depth === 0) {
      entries.push(source.slice(start, index));
      start = index + 1;
    }
  }

  entries.push(source.slice(start));
  return entries;
}

function findTopLevelColon(source) {
  let depth = 0;

  for (let index = 0; index < source.length; index += 1) {
    const skipped = skipIgnored(source, index);
    if (skipped !== index) {
      index = skipped - 1;
      continue;
    }

    const char = source[index];
    if (char === "{" || char === "[" || char === "(") depth += 1;
    else if (char === "}" || char === "]" || char === ")") depth = Math.max(0, depth - 1);
    else if (char === ":" && depth === 0) return index;
  }

  return -1;
}

function objectPropertyKey(source) {
  const trimmed = stripComments(source).trim();
  const identifier = trimmed.match(/^([A-Za-z_$][\w$]*)$/);
  if (identifier) return identifier[1];

  const quoted = trimmed.match(/^["']([^"']+)["']$/);
  return quoted?.[1] ?? null;
}

function normalizedPropertySource(property) {
  const stringValue = stringLiteralValue(property.value);
  if (stringValue !== null) {
    return `${property.name}: "${stringValue}"`;
  }

  const booleanValue = booleanLiteralValue(property.value);
  if (booleanValue !== null) {
    return `${property.name}: ${booleanValue ? "true" : "false"}`;
  }

  return `${property.name}: ${property.value}`;
}

function stringLiteralValue(source) {
  const trimmed = stripComments(source).trim();
  const quote = trimmed[0];
  if (quote !== '"' && quote !== "'") return null;

  let value = "";
  for (let index = 1; index < trimmed.length; index += 1) {
    const char = trimmed[index];
    if (char === "\\") {
      if (index + 1 < trimmed.length) {
        value += trimmed[index + 1];
        index += 1;
      }
      continue;
    }
    if (char === quote) {
      return trimmed.slice(index + 1).trim() === "" ? value : null;
    }
    value += char;
  }

  return null;
}

function booleanLiteralValue(source) {
  const trimmed = stripComments(source).trim();
  if (trimmed === "true") return true;
  if (trimmed === "false") return false;
  return null;
}

function maskCommentsAndStrings(source) {
  let masked = "";
  for (let index = 0; index < source.length; index += 1) {
    const skipped = skipIgnored(source, index);
    if (skipped !== index) {
      masked += " ".repeat(skipped - index);
      index = skipped - 1;
      continue;
    }
    masked += source[index];
  }
  return masked;
}

function stripComments(source) {
  let stripped = "";
  for (let index = 0; index < source.length; index += 1) {
    const blockEnd = source.startsWith("/*", index) ? source.indexOf("*/", index + 2) : -1;
    if (blockEnd !== -1) {
      stripped += " ".repeat(blockEnd + 2 - index);
      index = blockEnd + 1;
      continue;
    }

    if (source.startsWith("//", index)) {
      const lineEnd = source.indexOf("\n", index + 2);
      const end = lineEnd === -1 ? source.length : lineEnd;
      stripped += " ".repeat(end - index);
      index = end - 1;
      continue;
    }

    stripped += source[index];
  }
  return stripped;
}

function findMatchingDelimiterIndex(source, openIndex, open, close) {
  let depth = 0;

  for (let index = openIndex; index < source.length; index += 1) {
    const skipped = skipIgnored(source, index);
    if (skipped !== index) {
      index = skipped - 1;
      continue;
    }

    const char = source[index];
    if (char === open) depth += 1;
    if (char === close) depth -= 1;
    if (depth === 0) return index;
  }

  return -1;
}

function skipIgnored(source, start) {
  if (source.startsWith("//", start)) {
    const lineEnd = source.indexOf("\n", start + 2);
    return lineEnd === -1 ? source.length : lineEnd;
  }

  if (source.startsWith("/*", start)) {
    const blockEnd = source.indexOf("*/", start + 2);
    return blockEnd === -1 ? source.length : blockEnd + 2;
  }

  if (source[start] === "/" && isRegexLiteralStart(source, start)) {
    return findRegexEnd(source, start);
  }

  const quote = source[start];
  if (quote === '"' || quote === "'" || quote === "`") {
    return findStringEnd(source, start, quote);
  }

  return start;
}

function isRegexLiteralStart(source, start) {
  for (let index = start - 1; index >= 0; index -= 1) {
    if (/\s/.test(source[index])) continue;
    return "([{=,:;!?&|+-*%^~<>".includes(source[index]);
  }
  return true;
}

function findRegexEnd(source, start) {
  let escaped = false;
  let inCharacterClass = false;
  for (let index = start + 1; index < source.length; index += 1) {
    const char = source[index];
    if (escaped) {
      escaped = false;
      continue;
    }
    if (char === "\\") {
      escaped = true;
      continue;
    }
    if (char === "[") {
      inCharacterClass = true;
      continue;
    }
    if (char === "]") {
      inCharacterClass = false;
      continue;
    }
    if (char === "/" && !inCharacterClass) {
      let end = index + 1;
      while (/[A-Za-z]/.test(source[end] ?? "")) end += 1;
      return end;
    }
    if (char === "\n" || char === "\r") return start;
  }
  return source.length;
}

function findStringEnd(source, start, quote) {
  for (let index = start + 1; index < source.length; index += 1) {
    const char = source[index];
    if (char === "\\") {
      index += 1;
      continue;
    }
    if (char === quote) return index + 1;
  }
  return source.length;
}
