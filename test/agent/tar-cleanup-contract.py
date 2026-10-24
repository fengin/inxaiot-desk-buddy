"""镜像安装副本清理回归：执行真实 POSIX 函数，仅使用隔离文件与假 Docker/Compose。"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile


parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--shell", default="sh", help="POSIX 测试运行时")
parser.add_argument("--agent", type=Path, help="待测 Agent 文件；默认读取工程内嵌脚本")
parser.add_argument("--output", type=Path, help="保存 JSON 证据，文件必须尚不存在")
args = parser.parse_args()
agent_path = args.agent or Path(__file__).resolve().parents[2] / "src-tauri/resources/agent/edge-node-agent.sh"
agent_bytes = agent_path.read_bytes()
agent = agent_bytes.decode("utf-8")
cleanup = agent[agent.index("cleanup_loaded_image_archives() ("):agent.index("\nfind_release_src() {")]
install = agent[agent.index("install_release() {"):agent.index("\nbackup_current() {")]
service = agent[agent.index("service_upgrade() {"):agent.index("\nconfigured_service_image() {")]
environment = dict(os.environ)
if Path(args.shell).is_absolute():
    environment["PATH"] = str(Path(args.shell).parent) + os.pathsep + environment.get("PATH", "")
# 不允许 Git for Windows 静默把符号链接复制成普通文件，否则链接防护没有被验证。
environment["MSYS"] = (environment.get("MSYS", "") + " winsymlinks:nativestrict").strip()
subprocess.run([args.shell, "-n"], input=agent_bytes, env=environment, timeout=15, check=True)
events = r'''event() { printf '%s|%s|%s\n' "$1" "$2" "${3:-}"; }
docker() { printf 'unexpected Docker call\n' >&2; exit 99; }
'''
results = []


def execute(script: str, values, expected: int = 0):
    completed = subprocess.run(
        [args.shell, "-s"], input=("set -eu\n" + script).encode(),
        env=dict(environment, **values), stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=20,
    )
    assert completed.returncode == expected, (completed.returncode, expected, completed.stderr.decode(errors="replace"))
    return completed.stdout.decode("utf-8", errors="replace")


with tempfile.TemporaryDirectory(prefix="inx-agent-tar-contract-") as scratch:
    root = Path(scratch)
    physical_root = subprocess.check_output(
        [args.shell, "-c", 'cd "$1" && pwd -P', "fixture", str(root)],
        env=environment, universal_newlines=True, timeout=15,
    ).strip()

    def shell_path(path: Path):
        return physical_root + "/" + path.relative_to(root).as_posix()

    def write(path: Path, value: bytes = b"fixture"):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(value)
        return path

    def prepare(name: str, mode: str = "release"):
        base = root / name
        deploy = base / "deploy"
        directory = deploy / ("releases" if mode == "release" else "service-upgrades") / "current-task"
        package = write(directory / ("images/edge.tar" if mode == "release" else "service-image.tar"))
        keep = [write(directory / item) for item in [".env", "docker-compose.yml", "manifest.json", "checksums/sha256.txt", "images/edge.tag", "metadata.json"]]
        source = write(base / "input/source.tar", b"original archive")
        old = write(deploy / "releases/previous/images/old.tar", b"previous version")
        backup = write(base / "data/backup/database.db", b"backup evidence")
        values = {"DEPLOY_ROOT": shell_path(deploy), "RELEASE_VERSION": "current-task", "TASK_ID": "current-task", "CLEANUP_DIR": shell_path(directory)}
        call = 'cleanup_loaded_image_archives release "$CLEANUP_DIR"' if mode == "release" else 'cleanup_loaded_image_archives service "$CLEANUP_DIR" service-image.tar'
        return base, directory, package, keep + [source, old, backup], values, call

    for mode in ["release", "service"]:
        base, directory, package, keep, values, call = prepare("success-" + mode, mode)
        before = {path: path.read_bytes() for path in keep}
        output = execute(events + cleanup + "\n" + call, values)
        assert not package.exists() and "artifact_cleanup|success|" in output
        assert all(path.read_bytes() == body for path, body in before.items())
        assert execute(events + cleanup + "\n" + call, values) == ""
        results.append({"case": mode + "_success_preserves_source_metadata_old_version_and_backup", "passed": True})

    base, directory, package, keep, values, call = prepare("delete-denied")
    output = execute(events + cleanup + '\nrm() { return 1; }\n' + call, values)
    assert package.is_file() and "artifact_cleanup|warning|" in output
    assert all(path.is_file() for path in keep)
    results.append({"case": "delete_failure_is_warning_and_does_not_fail_deployment", "passed": True})

    for name, override in [
        ("wrong-release", {"RELEASE_VERSION": "another-task"}),
        ("traversal-root", {"DEPLOY_ROOT": physical_root + "/../outside"}),
        ("root-directory", {"DEPLOY_ROOT": "/"}),
        ("relative-root", {"DEPLOY_ROOT": "relative"}),
    ]:
        base, directory, package, keep, values, call = prepare(name)
        output = execute(events + cleanup + "\n" + call, dict(values, **override))
        assert package.is_file() and "artifact_cleanup|warning|" in output
        results.append({"case": name + "_does_not_delete", "passed": True})
    base, directory, package, keep, values, call = prepare("wrong-service-task", "service")
    output = execute(events + cleanup + "\n" + call, dict(values, TASK_ID="another-task"))
    assert package.is_file() and "artifact_cleanup|warning|" in output
    results.append({"case": "wrong_service_task_does_not_delete", "passed": True})

    # 使用原生文件系统链接；创建不成功时明确失败，不能降为普通副本后宣称通过。
    for kind in ["file", "images-directory", "task-directory"]:
        base, directory, package, keep, values, call = prepare("linked-" + kind)
        external = write(base / "external/edge.tar", b"external archive")
        if kind == "file":
            link, target = package, external
            package.unlink()
        elif kind == "images-directory":
            link, target = directory / "images", external.parent
            package.unlink()
            (link / "edge.tag").unlink()
            link.rmdir()
        else:
            link, target = directory, base / "external-task"
            directory.rename(target)
        execute('ln -s "$LINK_TARGET" "$LINK_PATH"\n[ -L "$LINK_PATH" ]', {
            "LINK_TARGET": shell_path(target), "LINK_PATH": shell_path(link),
        })
        output = execute(events + cleanup + "\n" + call, values)
        assert "artifact_cleanup|warning|" in output
        assert external.read_bytes() == b"external archive"
        if kind == "task-directory":
            assert (target / "images/edge.tar").is_file()
        link.unlink()
        results.append({"case": "symbolic_" + kind + "_preserves_external_files", "passed": True})

    # 执行真实安装/单服函数，桩仅替换外部依赖；记录清理函数是否被实际调用。
    cleanup_instrumented = cleanup.replace("cleanup_loaded_image_archives() (", "cleanup_loaded_image_archives_impl() (", 1)
    cleanup_instrumented += r'''
cleanup_loaded_image_archives() {
  printf 'cleanup\n' >> "$CLEANUP_CALLS"
  cleanup_loaded_image_archives_impl "$@"
}
'''
    stub = r'''
require_safe_release_version() { :; }
precheck() { :; }
prepare_dirs() { mkdir -p "$DATA_ROOT/config" "$DEPLOY_ROOT/releases"; }
current_release_dir() { printf '%s' "$CURRENT_RELEASE"; }
find_release_src() { printf '%s' "$RELEASE_SOURCE"; }
snapshot_release_images() { mkdir -p "$2"; }
restore_release_image_snapshot() { :; }
restore_image_reference() { :; }
load_image_archive_with_tag() { [ "$FAIL_STAGE" != load ]; }
stop_current_release() { :; }
wait_for_release() { [ "$FAIL_STAGE" != health ]; }
wait_for_service_state() { [ "$FAIL_STAGE" != health ]; }
tar() { :; }
ln() { :; }
mktemp() {
  if [ "${1:-}" = -d ]; then mkdir -p "$FIXTURE_DIR/tmp"; printf '%s' "$FIXTURE_DIR/tmp";
  else : > "$DATA_ROOT/config/.host-info.candidate"; printf '%s' "$DATA_ROOT/config/.host-info.candidate"; fi
}
compose() { [ "$FAIL_STAGE" != compose ]; }
rm() {
  if [ "$FAIL_STAGE" = cleanup_denied ] && [ "${1:-}" = -f ]; then return 1; fi
  command rm "$@"
}
rollback_release() { printf 'rollback-release\n' >> "$ROLLBACK_CALLS"; }
rollback_service_upgrade() { printf 'rollback-service\n' >> "$ROLLBACK_CALLS"; }
require_service_context() { printf '%s' "$CURRENT_RELEASE"; }
service_image_env_key() { printf '%s' APP_IMAGE; }
env_value() { printf '%s' repo/old:1; }
service_runtime_image_id() { printf '%s' sha256:old; }
image_id_for_reference() { printf '%s' sha256:new; }
service_upgrade_dir() { printf '%s' "$DEPLOY_ROOT/service-upgrades/$TASK_ID"; }
prepare_service_compose() { cp "$1" "$2"; }
replace_env_value() { printf '%s=%s\n' "$2" "$3" > "$1"; }
json_escape() { printf '%s' "$1"; }
fail() { exit "${3:-1}"; }
'''
    for mode, exits in [("install", {"success": 0, "cleanup_denied": 0, "load": 36, "compose": 39, "health": 41}),
                        ("service", {"success": 0, "cleanup_denied": 0, "load": 73, "compose": 84, "health": 86})]:
        for failure, expected in exits.items():
            base = root / ("flow-" + mode + "-" + failure)
            deploy, data = base / "deploy", base / "data"
            current = deploy / "releases/previous"
            write(current / "docker-compose.yml")
            write(current / ".env", b"APP_IMAGE=repo/old:1\n")
            write(current / "manifest.json", b"{}")
            write(data / "config/host-info.json", b"old host")
            release_source = base / "release-source"
            write(release_source / "manifest.json", b"{}")
            write(release_source / "images/edge.tar")
            write(release_source / "images/edge.tag", b"repo/new:1")
            inputs = {key: write(base / "inputs" / name) for key, name in [
                ("REMOTE_PACKAGE", "release.tar"), ("REMOTE_IMAGE", "service-image.tar"),
                ("REMOTE_ENV", "app.env"), ("REMOTE_HOST_INFO", "host-info.json"),
                ("REMOTE_COMPOSE", "docker-compose.yml"),
            ]}
            calls, rollback_calls = base / "cleanup.calls", base / "rollback.calls"
            values = {"DEPLOY_ROOT": shell_path(deploy), "DATA_ROOT": shell_path(data),
                      "CURRENT_RELEASE": shell_path(current), "RELEASE_SOURCE": shell_path(release_source),
                      "FIXTURE_DIR": shell_path(base), "CLEANUP_CALLS": shell_path(calls),
                      "ROLLBACK_CALLS": shell_path(rollback_calls), "FAIL_STAGE": failure,
                      "RELEASE_VERSION": "new-version", "RELEASE_FINGERPRINT": "", "TASK_ID": "current-task",
                      "SERVICE_IMAGE": "repo/new:1", "SERVICE_NAME": "edge",
                      **{key: shell_path(value) for key, value in inputs.items()}}
            function = install if mode == "install" else service
            invocation = "install_release" if mode == "install" else "service_upgrade"
            output = execute(events + cleanup_instrumented + function + stub + "\n" + invocation, values, expected)
            assert calls.exists() == (failure in ["success", "cleanup_denied"]), (mode, failure, output)
            if failure in ["success", "cleanup_denied"]:
                archive = deploy / ("releases/new-version/images/edge.tar" if mode == "install" else "service-upgrades/current-task/service-image.tar")
                assert archive.exists() == (failure == "cleanup_denied")
                assert ("artifact_cleanup|warning|" if failure == "cleanup_denied" else "artifact_cleanup|success|") in output
                assert ("install|success|" if mode == "install" else "service_upgrade|success|") in output
            else:
                assert "artifact_cleanup|" not in output
            assert all(path.is_file() for path in inputs.values())
            assert (current / "docker-compose.yml").is_file()
            assert rollback_calls.exists() == (failure in ["compose", "health"])
            results.append({"case": mode + "_" + failure + "_calls_cleanup_only_after_health_success", "passed": True})

report = {"passed": True, "caseCount": len(results), "agentSha256": hashlib.sha256(agent_bytes).hexdigest(),
          "scope": "隔离临时目录；真实shell函数；Docker和Compose为桩；未连接设备", "cases": results}
if args.output:
    with args.output.open("x", encoding="utf-8") as output:
        json.dump(report, output, ensure_ascii=False, indent=2)
print(json.dumps(report, ensure_ascii=False))
