import { chmodSync, copyFileSync, lstatSync, mkdirSync, readdirSync, realpathSync, rmSync, statSync, unlinkSync } from 'node:fs';
import { isAbsolute, join, relative } from 'node:path';

const inside = (root, path) => {
  const part = relative(root, path);
  return part === '' || (!part.startsWith('..') && !isAbsolute(part));
};

// jlink 的许可证可能是只读文件或相互引用的链接。逐文件替换，避免递归 cp
// 覆盖只读文件，以及把指向构建机 JDK 的链接留在发布包中；不改源 JDK 权限。
export function copyJavaLegalNotices(javaHome, runtimeHome) {
  const source = realpathSync(join(javaHome, 'legal'));
  const runtime = realpathSync(runtimeHome);
  if (inside(source, runtime) || inside(runtime, source)) throw new Error('Java 许可证源目录与运行时目录必须分开');
  const destination = join(runtime, 'legal');
  const copy = (from, to, parents = new Set()) => {
    const resolved = realpathSync(from);
    if (!inside(source, resolved)) throw new Error(`Java 许可证链接超出源目录：${from}`);
    const sourceStat = statSync(resolved);
    const existing = lstatSync(to, { throwIfNoEntry: false });
    if (sourceStat.isDirectory()) {
      if (parents.has(resolved)) throw new Error(`Java 许可证目录存在循环链接：${from}`);
      if (existing && (!existing.isDirectory() || existing.isSymbolicLink())) throw new Error(`Java 许可证目标目录无效：${to}`);
      if (!existing) mkdirSync(to);
      if (!inside(runtime, realpathSync(to))) throw new Error(`Java 许可证目标超出运行时目录：${to}`);
      const ancestors = new Set([...parents, resolved]);
      for (const name of readdirSync(resolved)) copy(join(resolved, name), join(to, name), ancestors);
      if (!existing) chmodSync(to, sourceStat.mode & 0o777);
    } else if (sourceStat.isFile()) {
      if (existing?.isDirectory()) throw new Error(`Java 许可证目标应为文件：${to}`);
      // 删除的是已核对父目录中的目标文件，不跟随目标链接，也不修改源权限。
      if (existing?.isSymbolicLink()) unlinkSync(to);
      else if (existing) rmSync(to, { force: true });
      copyFileSync(resolved, to);
      chmodSync(to, sourceStat.mode & 0o777);
    } else throw new Error(`Java 许可证不是普通文件：${from}`);
  };
  copy(source, destination);
}
