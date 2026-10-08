import { effectiveScreenMac, normalizeScreenMac, validateLocalScreen } from "@/shared/model/screen";
import type { LocalScreenInput, SmartScreen } from "@/shared/model/screen";
import { buildProjectSpaceTree, projectSpacePath, resolveProjectSpacePath } from "@/shared/model/projectSpace";
import type { ProjectSpaceNode } from "@/shared/model/projectSpace";

type SpaceMatch = ReturnType<typeof resolveProjectSpacePath>;
export interface ScreenImportRow {
  line: number;
  input: LocalScreenInput;
  originalSpacePath: string;
  /** undefined 沿用文件路径；null 表示人员明确选择待定空间。 */
  spaceOverrideId?: string | null;
  spaceStatus: SpaceMatch["status"];
  matchedSpacePath: string;
  spaceError: string;
  formatError: string;
  error: string;
  warning: string;
}

function csvRows(text: string) {
  const rows: { line: number; cells: string[] }[] = [];
  let cells: string[] = [], field = "", quoted = false, line = 1, startLine = 1;
  const source = text.replace(/^\uFEFF/, "").replace(/\r\n?/g, "\n");
  for (let i = 0; i < source.length; i++) {
    const char = source[i];
    if (char === '"') {
      if (quoted && source[i + 1] === '"') { field += '"'; i++; } else quoted = !quoted;
    } else if (!quoted && (char === "," || char === "\t")) { cells.push(field.trim()); field = ""; }
    else if (!quoted && char === "\n") {
      cells.push(field.trim()); if (cells.some(Boolean)) rows.push({ line: startLine, cells });
      cells = []; field = ""; startLine = line + 1;
    } else field += char;
    if (char === "\n") line++;
  }
  if (quoted) throw new Error("CSV 引号未闭合，请检查文件内容");
  cells.push(field.trim()); if (cells.some(Boolean)) rows.push({ line: startLine, cells });
  return rows;
}

function resolveRowSpace(row: ScreenImportRow, spaces: readonly ProjectSpaceNode[], available: boolean): SpaceMatch {
  if (row.spaceOverrideId === undefined) return resolveProjectSpacePath(row.originalSpacePath, spaces, available);
  if (row.spaceOverrideId === null) return { status: "empty", path: "", error: "" };
  if (!available) return { status: "unavailable", path: "", error: "当前项目空间目录不可用，无法核实所选空间" };
  const path = projectSpacePath(spaces, row.spaceOverrideId);
  return path ? { status: "matched", spaceId: row.spaceOverrideId, path, error: "" }
    : { status: "missing", path: "", error: "人工选择的空间已失效，请从当前项目目录重新选择" };
}

/** 每次目录或资产变化后重新核对；人工选择保留实际 ID，不重新按同名节点猜测。 */
export function revalidateScreenInventory(rows: readonly ScreenImportRow[], existing: readonly SmartScreen[], spaces: readonly ProjectSpaceNode[], available = true): ScreenImportRow[] {
  const seenIps = new Set<string>(), seenMacs = new Set<string>();
  return rows.map((row) => {
    const match = resolveRowSpace(row, spaces, available);
    const matchedPath = match.status === "matched" ? match.path : "";
    const input = { ...row.input, spaceId: match.spaceId ?? null, spacePath: matchedPath };
    const errors = [...validateLocalScreen(input), row.formatError, match.error].filter(Boolean);
    const warnings: string[] = [];
    const mac = normalizeScreenMac(input.mac);
    if (input.ip && seenIps.has(input.ip)) errors.push("文件内 IP 重复");
    if (existing.some((screen) => screen.source === "local" && screen.ip === input.ip)) errors.push("本机已存在相同 IP");
    if (existing.some((screen) => screen.source === "platform" && screen.ip === input.ip)) warnings.push("与平台 IP 相同，导入后提示核对合并");
    if (mac && seenMacs.has(mac)) warnings.push("文件内 MAC 重复，请核对是否为同一台设备");
    if (mac && existing.some((screen) => normalizeScreenMac(effectiveScreenMac(screen)) === mac)) warnings.push("已有记录使用相同 MAC，请核对设备身份");
    if (input.ip) seenIps.add(input.ip); if (mac) seenMacs.add(mac);
    return { ...row, input, spaceStatus: match.status, matchedSpacePath: matchedPath, spaceError: match.error, error: errors.join("；"), warning: warnings.join("；") };
  });
}

/** 新清单以完整空间路径关联当前项目目录，不接收旧楼幢/楼层文本列。 */
export function parseScreenInventory(text: string, existing: readonly SmartScreen[], spaces: readonly ProjectSpaceNode[] = [], available = true): ScreenImportRow[] {
  const records = csvRows(text);
  if (!records.length) return [];
  const titles = records[0]!.cells.map((cell) => cell.replace(/\s/g, "").toLowerCase());
  const hasHeader = titles.some((cell) => ["名称", "屏名称", "name", "空间路径", "楼幢", "楼层"].includes(cell));
  const aliases = [["名称", "屏名称", "name"], ["ip", "ip地址"], ["尺寸", "size"], ["mac", "mac地址"], ["空间路径", "spacepath"], ["位置", "安装位置", "详细位置", "location"]];
  let indices = [0, 1, 2, 3, 4, 5];
  if (hasHeader) {
    if (titles.some((title) => ["楼幢", "楼层", "building", "floor"].includes(title))) throw new Error("请使用新清单模板：将楼幢、楼层改为一个完整的“空间路径”列");
    indices = aliases.map((names) => titles.findIndex((title) => names.includes(title)));
    if ([0, 1, 2, 4].some((index) => indices[index] === -1)) throw new Error("清单必须包含名称、IP、尺寸、空间路径列；空间路径可留空表示待定空间");
    if (aliases.some((names) => titles.filter((title) => names.includes(title)).length > 1)) throw new Error("清单存在重复含义的列，请每个字段只保留一列");
  }
  const rows = (hasHeader ? records.slice(1) : records).map(({ line, cells }): ScreenImportRow => {
    const [name, ip, sizeValue, mac, originalSpacePath, location] = indices.map((index) => cells[index] ?? "");
    const size = (sizeValue ?? "").replace(/寸|inch|[ -]/gi, "");
    const errors: string[] = [];
    if (size && !["4", "10", "待确认", "unknown"].includes(size)) errors.push("尺寸仅支持4、10或待确认");
    if (cells.length > (hasHeader ? titles.length : 6)) errors.push("列数超出模板，请使用名称、IP、尺寸、MAC、空间路径、安装位置六列，并用引号包裹含逗号的内容");
    return { line, input: { name: name ?? "", ip: ip ?? "", size: size === "4" ? "4" : size === "10" ? "10" : "unknown", mac: mac ?? "", location: location ?? "" },
      originalSpacePath: originalSpacePath ?? "", spaceStatus: "empty", matchedSpacePath: "", spaceError: "", formatError: errors.join("；"), error: "", warning: "" };
  });
  return revalidateScreenInventory(rows, existing, spaces, available);
}

export function inventorySpacePaths(spaces: readonly ProjectSpaceNode[], available = true): string[] {
  if (!available) return [];
  const paths: string[] = [];
  const collect = (nodes: ReturnType<typeof buildProjectSpaceTree>) => {
    for (const node of nodes) { paths.push(node.path.split("/").map((part) => part.trim()).join("/")); if (node.children) collect(node.children); }
  };
  collect(buildProjectSpaceTree(spaces));
  const counts = new Map<string, number>();
  for (const path of paths) counts.set(path, (counts.get(path) ?? 0) + 1);
  return paths.filter((path) => counts.get(path) === 1);
}

export function inventorySpacePathsCsv(paths: readonly string[]): string {
  return `\uFEFF空间路径\r\n${paths.map((path) => `"${path.split("/").map((part) => part.trim()).join("/").replace(/"/g, '""')}"`).join("\r\n")}\r\n`;
}
