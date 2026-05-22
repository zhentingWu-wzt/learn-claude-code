import * as fs from "fs";
import * as path from "path";
import type {
  AgentVersion,
  VersionDiff,
  DocContent,
  VersionIndex,
} from "../src/types/agent-data";
import { VERSION_META, VERSION_ORDER, LEARNING_PATH } from "../src/lib/constants";

// Resolve paths relative to this script's location (web/scripts/)
const WEB_DIR = path.resolve(__dirname, "..");
const REPO_ROOT = path.resolve(WEB_DIR, "..");
const AGENTS_DIR = path.join(REPO_ROOT, "agents-rs");
const DOCS_DIR = path.join(REPO_ROOT, "docs");
const OUT_DIR = path.join(WEB_DIR, "src", "data", "generated");

// Map Rust directory names to version IDs
// s01_agent_loop/src/main.rs -> s01
// s02_tool_use/src/main.rs -> s02
// s_full/src/main.rs -> s_full (reference agent, typically skipped)
function filenameToVersionId(dirName: string): string | null {
  if (dirName === "s_full") return null;
  if (dirName === "harness") return null;
  if (dirName === "target") return null;

  const match = dirName.match(/^(s\d+[a-c]?)_/);
  if (!match) return null;
  return match[1];
}

// Extract structs/enums from Rust source
function extractClasses(
  lines: string[]
): { name: string; startLine: number; endLine: number }[] {
  const classes: { name: string; startLine: number; endLine: number }[] = [];
  const structPattern = /^(?:pub\s+)?struct\s+(\w+)/;
  const enumPattern = /^(?:pub\s+)?enum\s+(\w+)/;

  for (let i = 0; i < lines.length; i++) {
    const m = lines[i].match(structPattern) || lines[i].match(enumPattern);
    if (m) {
      const name = m[1];
      const startLine = i + 1;
      // Find end: next item at indent 0, or EOF
      let endLine = lines.length;
      for (let j = i + 1; j < lines.length; j++) {
        if (
          lines[j].match(/^(?:pub\s+)?struct\s/) ||
          lines[j].match(/^(?:pub\s+)?enum\s/) ||
          lines[j].match(/^(?:pub\s+)?fn\s/) ||
          lines[j].match(/^impl\s/) ||
          lines[j].match(/^\/\/!\s/) ||
          (lines[j].match(/^\S/) && lines[j].trim() !== "" && !lines[j].startsWith("//") && !lines[j].startsWith("#"))
        ) {
          endLine = j;
          break;
        }
      }
      classes.push({ name, startLine, endLine });
    }
  }
  return classes;
}

// Extract top-level functions from Rust source
function extractFunctions(
  lines: string[]
): { name: string; signature: string; startLine: number }[] {
  const functions: { name: string; signature: string; startLine: number }[] = [];
  const funcPattern = /^(?:pub\s+)?(?:async\s+)?fn\s+(\w+)\((.*?)\)/;

  for (let i = 0; i < lines.length; i++) {
    const m = lines[i].match(funcPattern);
    if (m) {
      const prefix = lines[i].match(/^(pub\s+)?(async\s+)?fn\s/)?.[0] || "fn ";
      functions.push({
        name: m[1],
        signature: `${prefix}${m[1]}(${m[2]})`,
        startLine: i + 1,
      });
    }
  }
  return functions;
}

// Extract tool names from Rust source
// Looks for "name": "tool_name", name: "...".into(), or name: "...".to_string() patterns
function extractTools(source: string): string[] {
  const tools = new Set<string>();
  let m;
  // JSON-style: "name": "tool_name"
  const jsonPattern = /"name"\s*:\s*"(\w+)"/g;
  while ((m = jsonPattern.exec(source)) !== null) {
    tools.add(m[1]);
  }
  // Rust struct field: name: "tool_name".into()
  const intoPattern = /name:\s*"(\w+)"\.into\(\)/g;
  while ((m = intoPattern.exec(source)) !== null) {
    tools.add(m[1]);
  }
  // Rust struct field: name: "tool_name".to_string()
  const toStringPattern = /name:\s*"(\w+)"\.to_string\(\)/g;
  while ((m = toStringPattern.exec(source)) !== null) {
    tools.add(m[1]);
  }
  // Dispatch match arms: "tool_name" =>
  const dispatchPattern = /"(\w+)"\s*=>/g;
  while ((m = dispatchPattern.exec(source)) !== null) {
    // Filter out non-tool keywords
    const name = m[1];
    if (!["true", "false", "Some", "None", "Ok", "Err"].includes(name)) {
      tools.add(name);
    }
  }
  return Array.from(tools);
}

// Count non-blank, non-comment lines (Rust // comments)
function countLoc(lines: string[]): number {
  return lines.filter((line) => {
    const trimmed = line.trim();
    return trimmed !== "" && !trimmed.startsWith("//");
  }).length;
}

// Detect locale from subdirectory path
// docs/en/s01-the-agent-loop.md -> "en"
// docs/zh/s01-the-agent-loop.md -> "zh"
// docs/ja/s01-the-agent-loop.md -> "ja"
function detectLocale(relPath: string): "en" | "zh" | "ja" {
  if (relPath.startsWith("zh/") || relPath.startsWith("zh\\")) return "zh";
  if (relPath.startsWith("ja/") || relPath.startsWith("ja\\")) return "ja";
  return "en";
}

// Extract version from doc filename (e.g., "s01-the-agent-loop.md" -> "s01")
function extractDocVersion(filename: string): string | null {
  const m = filename.match(/^(s\d+[a-c]?)-/);
  return m ? m[1] : null;
}

// Main extraction
function main() {
  console.log("Extracting content from agents and docs...");
  console.log(`  Repo root: ${REPO_ROOT}`);
  console.log(`  Agents dir: ${AGENTS_DIR}`);
  console.log(`  Docs dir: ${DOCS_DIR}`);

  // Skip extraction if source directories don't exist (e.g. Vercel build).
  // Pre-committed generated data will be used instead.
  if (!fs.existsSync(AGENTS_DIR)) {
    console.log("  Agents directory not found, skipping extraction.");
    console.log("  Using pre-committed generated data.");
    return;
  }

  // 1. Read all agent directories (Rust workspace: sNN_name/src/main.rs)
  const agentDirs = fs
    .readdirSync(AGENTS_DIR)
    .filter((f) => f.startsWith("s") && fs.statSync(path.join(AGENTS_DIR, f)).isDirectory());

  console.log(`  Found ${agentDirs.length} agent directories`);

  const versions: AgentVersion[] = [];

  for (const dirName of agentDirs) {
    const versionId = filenameToVersionId(dirName);
    if (!versionId) {
      console.warn(`  Skipping ${dirName}: could not determine version ID`);
      continue;
    }

    const mainPath = path.join(AGENTS_DIR, dirName, "src", "main.rs");
    if (!fs.existsSync(mainPath)) {
      console.warn(`  Skipping ${dirName}: no src/main.rs found`);
      continue;
    }

    const source = fs.readFileSync(mainPath, "utf-8");
    const lines = source.split("\n");

    const meta = VERSION_META[versionId];
    const classes = extractClasses(lines);
    const functions = extractFunctions(lines);
    const tools = extractTools(source);
    const loc = countLoc(lines);

    versions.push({
      id: versionId,
      filename: `${dirName}/src/main.rs`,
      title: meta?.title ?? versionId,
      subtitle: meta?.subtitle ?? "",
      loc,
      tools,
      newTools: [], // computed after all versions are loaded
      coreAddition: meta?.coreAddition ?? "",
      keyInsight: meta?.keyInsight ?? "",
      classes,
      functions,
      layer: meta?.layer ?? "tools",
      source,
    });
  }

  // Sort versions according to VERSION_ORDER
  const orderMap = new Map(VERSION_ORDER.map((v, i) => [v, i]));
  versions.sort(
    (a, b) => (orderMap.get(a.id as any) ?? 99) - (orderMap.get(b.id as any) ?? 99)
  );

  // 2. Compute newTools for each version
  for (let i = 0; i < versions.length; i++) {
    const prev = i > 0 ? new Set(versions[i - 1].tools) : new Set<string>();
    versions[i].newTools = versions[i].tools.filter((t) => !prev.has(t));
  }

  // 3. Compute diffs between adjacent versions in LEARNING_PATH
  const diffs: VersionDiff[] = [];
  const versionMap = new Map(versions.map((v) => [v.id, v]));

  for (let i = 1; i < LEARNING_PATH.length; i++) {
    const fromId = LEARNING_PATH[i - 1];
    const toId = LEARNING_PATH[i];
    const fromVer = versionMap.get(fromId);
    const toVer = versionMap.get(toId);

    if (!fromVer || !toVer) continue;

    const fromClassNames = new Set(fromVer.classes.map((c) => c.name));
    const fromFuncNames = new Set(fromVer.functions.map((f) => f.name));
    const fromToolNames = new Set(fromVer.tools);

    diffs.push({
      from: fromId,
      to: toId,
      newClasses: toVer.classes
        .map((c) => c.name)
        .filter((n) => !fromClassNames.has(n)),
      newFunctions: toVer.functions
        .map((f) => f.name)
        .filter((n) => !fromFuncNames.has(n)),
      newTools: toVer.tools.filter((t) => !fromToolNames.has(t)),
      locDelta: toVer.loc - fromVer.loc,
    });
  }

  // 4. Read doc files from locale subdirectories (en/, zh/, ja/)
  const docs: DocContent[] = [];

  if (fs.existsSync(DOCS_DIR)) {
    const localeDirs = ["en", "zh", "ja"];
    let totalDocFiles = 0;

    for (const locale of localeDirs) {
      const localeDir = path.join(DOCS_DIR, locale);
      if (!fs.existsSync(localeDir)) continue;

      const docFiles = fs
        .readdirSync(localeDir)
        .filter((f) => f.endsWith(".md"));

      totalDocFiles += docFiles.length;

      for (const filename of docFiles) {
        const version = extractDocVersion(filename);
        if (!version) {
          console.warn(`  Skipping doc ${locale}/${filename}: could not determine version`);
          continue;
        }

        const filePath = path.join(localeDir, filename);
        const content = fs.readFileSync(filePath, "utf-8");

        const titleMatch = content.match(/^#\s+(.+)$/m);
        const title = titleMatch ? titleMatch[1] : filename;

        docs.push({ version, locale: locale as "en" | "zh" | "ja", title, content });
      }
    }

    console.log(`  Found ${totalDocFiles} doc files across ${localeDirs.length} locales`);
  } else {
    console.warn(`  Docs directory not found: ${DOCS_DIR}`);
  }

  // 5. Write output
  fs.mkdirSync(OUT_DIR, { recursive: true });

  const index: VersionIndex = { versions, diffs };
  const indexPath = path.join(OUT_DIR, "versions.json");
  fs.writeFileSync(indexPath, JSON.stringify(index, null, 2));
  console.log(`  Wrote ${indexPath}`);

  const docsPath = path.join(OUT_DIR, "docs.json");
  fs.writeFileSync(docsPath, JSON.stringify(docs, null, 2));
  console.log(`  Wrote ${docsPath}`);

  // Summary
  console.log("\nExtraction complete:");
  console.log(`  ${versions.length} versions`);
  console.log(`  ${diffs.length} diffs`);
  console.log(`  ${docs.length} docs`);
  for (const v of versions) {
    console.log(
      `    ${v.id}: ${v.loc} LOC, ${v.tools.length} tools, ${v.classes.length} classes, ${v.functions.length} functions`
    );
  }
}

main();
