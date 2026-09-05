const WINDOWS_EXTENDED_PATH_PREFIX = "\\\\?\\";
const WINDOWS_EXTENDED_UNC_PREFIX = "\\\\?\\UNC\\";

export function formatDisplayLocalPath(path?: string | null): string {
  if (!path) return "";
  if (path.startsWith(WINDOWS_EXTENDED_UNC_PREFIX)) {
    return "\\\\" + path.slice(WINDOWS_EXTENDED_UNC_PREFIX.length);
  }
  if (path.startsWith(WINDOWS_EXTENDED_PATH_PREFIX)) {
    return path.slice(WINDOWS_EXTENDED_PATH_PREFIX.length);
  }
  return path;
}
