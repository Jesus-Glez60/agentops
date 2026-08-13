// Maps a file extension to the Prism grammar name prism-react-renderer
// bundles by default -- just enough coverage for what this codebase (and
// most repos it'll scan) actually contains.
const EXTENSION_TO_LANGUAGE: Record<string, string> = {
  rs: "rust",
  ts: "tsx",
  tsx: "tsx",
  js: "jsx",
  jsx: "jsx",
  mjs: "jsx",
  py: "python",
  go: "go",
  rb: "ruby",
  java: "java",
  json: "json",
  yaml: "yaml",
  yml: "yaml",
  toml: "toml",
  md: "markdown",
  sh: "bash",
  bash: "bash",
  sql: "sql",
  css: "css",
  html: "markup",
};

export function languageFromPath(path: string | null | undefined): string | undefined {
  if (!path) return undefined;
  const ext = path.split(".").pop()?.toLowerCase();
  if (!ext) return undefined;
  return EXTENSION_TO_LANGUAGE[ext];
}
