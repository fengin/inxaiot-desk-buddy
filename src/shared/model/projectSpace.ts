/** 项目空间目录；层级由父子关系决定，不限制区域嵌套深度。 */
export interface ProjectSpaceNode {
  id: string;
  name: string;
  parentId?: string;
  kind: "building" | "floor" | "area" | "other";
}

export interface ProjectSpaceOption {
  value: string;
  label: string;
  path: string;
  children?: ProjectSpaceOption[];
}

function pathResolver(spaces: readonly ProjectSpaceNode[]) {
  const nodes = new Map<string, ProjectSpaceNode>();
  const duplicates = new Set<string>();
  for (const node of spaces) {
    if (nodes.has(node.id)) duplicates.add(node.id);
    nodes.set(node.id, node);
  }
  const cache = new Map<string, ProjectSpaceNode[] | undefined>();
  return (id: string): ProjectSpaceNode[] | undefined => {
    if (cache.has(id)) return cache.get(id);
    const path: ProjectSpaceNode[] = [], seen = new Set<string>();
    let current: string | undefined = id;
    while (current && current !== "0") {
      const node = nodes.get(current);
      if (!node || !node.name.trim() || duplicates.has(current) || seen.has(current)) {
        cache.set(id, undefined);
        return undefined;
      }
      seen.add(current); path.push(node); current = node.parentId;
    }
    const result = path.length ? path.reverse() : undefined;
    cache.set(id, result);
    return result;
  };
}

export function getProjectSpacePath(spaces: readonly ProjectSpaceNode[], id?: string | null): ProjectSpaceNode[] | undefined {
  return id ? pathResolver(spaces)(id) : undefined;
}

const pathText = (path: readonly ProjectSpaceNode[]) => path.map((node) => node.name.trim()).join("/");

export function projectSpacePath(spaces: readonly ProjectSpaceNode[], id?: string | null): string {
  const path = getProjectSpacePath(spaces, id);
  return path ? pathText(path) : "";
}

export function buildProjectSpaceTree(spaces: readonly ProjectSpaceNode[]): ProjectSpaceOption[] {
  const resolve = pathResolver(spaces), options = new Map<string, ProjectSpaceOption>();
  for (const node of spaces) {
    const path = resolve(node.id);
    if (path) options.set(node.id, { value: node.id, label: node.name, path: pathText(path) });
  }
  const roots: ProjectSpaceOption[] = [];
  for (const node of spaces) {
    const option = options.get(node.id);
    if (!option) continue;
    const parent = node.parentId ? options.get(node.parentId) : undefined;
    if (parent) (parent.children ??= []).push(option);
    else roots.push(option);
  }
  return roots;
}

export interface ProjectSpaceResolution {
  status: "matched" | "empty" | "missing" | "ambiguous" | "unavailable";
  spaceId?: string;
  path: string;
  error: string;
}

function normalizedPath(value: string): string {
  return value.split("/").map((part) => part.trim()).join("/");
}

export function resolveProjectSpacePath(text: string, spaces: readonly ProjectSpaceNode[], available = true): ProjectSpaceResolution {
  const input = text.trim();
  if (!input) return { status: "empty", path: "", error: "" };
  if (!available) return { status: "unavailable", path: input, error: "空间目录暂不可用，请恢复后重新匹配或明确设为待定空间" };
  const resolve = pathResolver(spaces), normalized = normalizedPath(input);
  const matches = spaces.filter((node) => {
    const path = resolve(node.id);
    return path && normalizedPath(pathText(path)) === normalized;
  });
  if (!matches.length) return { status: "missing", path: input, error: "空间路径不存在，请填写完整路径或从空间树重新选择" };
  if (matches.length !== 1) return { status: "ambiguous", path: input, error: `空间路径对应 ${matches.length} 个节点，请核实后从空间树明确选择` };
  return { status: "matched", spaceId: matches[0]!.id, path: pathText(resolve(matches[0]!.id)!), error: "" };
}

/** 一次遍历生成无歧义的完整路径，供各业务导入模板共用。 */
export function uniqueProjectSpacePaths(spaces: readonly ProjectSpaceNode[], available = true): string[] {
  if (!available) return [];
  const paths: string[] = [];
  const collect = (nodes: readonly ProjectSpaceOption[]) => {
    for (const node of nodes) { paths.push(normalizedPath(node.path)); if (node.children) collect(node.children); }
  };
  collect(buildProjectSpaceTree(spaces));
  const counts = new Map<string, number>();
  for (const path of paths) counts.set(path, (counts.get(path) ?? 0) + 1);
  return paths.filter(path => counts.get(path) === 1);
}

export function projectSpacePathsCsv(paths: readonly string[]): string {
  return `\uFEFF空间路径\r\n${paths.map(path => `"${normalizedPath(path).replace(/"/g, '""')}"`).join("\r\n")}\r\n`;
}
