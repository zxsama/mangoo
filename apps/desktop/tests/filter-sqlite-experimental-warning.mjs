const SQLITE_WARNING_NAME = "ExperimentalWarning";
const SQLITE_WARNING_MESSAGE = "SQLite is an experimental feature and might change at any time";

process.on("warning", (warning) => {
  const message = warning instanceof Error ? warning.message : String(warning);
  if (warning.name !== SQLITE_WARNING_NAME || message !== SQLITE_WARNING_MESSAGE) {
    const stack = warning.stack ?? `${warning.name}: ${message}`;
    process.stderr.write(`(node:${process.pid}) ${stack}\n`);
  }
});
