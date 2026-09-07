"""Agent 单服配置契约回归：只用临时文本和假 Compose，不连接 Docker 或业务目录。"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--shell', default='sh', help='POSIX测试运行时；Windows可指定Git for Windows的sh.exe')
parser.add_argument('--output', help='保存JSON证据（目标文件必须不存在）')
arguments = parser.parse_args()
shell = arguments.shell
test_environment = dict(os.environ)
if Path(shell).is_absolute():
    test_environment['PATH'] = str(Path(shell).parent) + os.pathsep + test_environment.get('PATH', '')
agent = globals().get('AGENT_TEXT')
if agent is None:
    agent = (Path(__file__).resolve().parents[2] / 'src-tauri/resources/agent/edge-node-agent.sh').read_text(encoding='utf-8')
subprocess.run([shell, '-n'], input=agent.encode('utf-8'), env=test_environment, timeout=15, check=True)
prepare = agent[agent.index('prepare_service_compose() {'):agent.index('\nrollback_service_upgrade() {')]
rollback = agent[agent.index('rollback_service_upgrade() {'):agent.index('\nservice_upgrade() {')]
rollback_full = agent[agent.index('rollback_release() {'):agent.index('\ninstall_release() {')]
install_full = agent[agent.index('install_release() {'):agent.index('\nbackup_current() {')]
load_image = agent[agent.index('load_image_archive_with_tag() {'):agent.index('\nfind_release_src() {')]
verify_service = agent[agent.index('verify_service_state() {'):agent.index('\n# 只兼容转换目标服务')]
verify_release_services = agent[agent.index('verify_runtime_containers() {'):agent.index('\nrollback_release() {')]
base = 'services:\n  emqx:\n    image: emqx:1\n  device-edge:\n    image: edge:old\n    labels:\n      image: unchanged\n  device-edge-web:\n    image: web:1\n'
cases = [
    ('literal', base, True),
    ('variable', base.replace('edge:old', '${DEVICE_EDGE_IMAGE}'), True),
    ('default', base.replace('edge:old', '${DEVICE_EDGE_IMAGE:-edge:old}'), True),
    ('required', base.replace('edge:old', '${DEVICE_EDGE_IMAGE:?required}'), True),
    ('short_variable', base.replace('edge:old', '$DEVICE_EDGE_IMAGE'), True),
    ('quoted_image', base.replace('image: edge:old', 'image: "edge:old" # keep'), True),
    ('quoted_service', base.replace('  device-edge:', '  "device-edge":'), True),
    ('single_quotes', base.replace('  device-edge:', "  'device-edge':").replace('image: edge:old', "image: 'edge:old'"), True),
    ('four_space_indent', '\n'.join(('  ' + line if line.startswith(' ') else line) for line in base.split('\n')), True),
    ('wrong_image', base.replace('image: edge:old', 'image: other:1'), False),
    ('missing_target', base.replace('  device-edge:', '  different:'), False),
    ('duplicate_image', base.replace('    image: edge:old', '    image: edge:old\n    image: edge:old'), False),
    ('duplicate_service', base + '  device-edge:\n    image: edge:old\n', False),
    ('duplicate_section', base + 'services:\n  other:\n    image: other:1\n', False),
    ('alias', base.replace('    image: edge:old', '    image: *shared'), False),
    ('flow', 'services: {device-edge: {image: "edge:old"}}\n', False),
]
results = []
with tempfile.TemporaryDirectory(prefix='inxaiot-compose-contract-', dir=Path(__file__).resolve().parent) as scratch:
    root = Path(scratch)
    for name, content, success in cases:
        source, output = root / (name + '.yml'), root / (name + '.after.yml')
        source.write_bytes(content.encode('utf-8'))
        completed = subprocess.run([shell, '-c', prepare + '\nSERVICE_NAME=device-edge\nprepare_service_compose "$1" "$2" DEVICE_EDGE_IMAGE edge:old', 'fixture', source.as_posix(), output.as_posix()], stdout=subprocess.PIPE, stderr=subprocess.PIPE, universal_newlines=True, env=test_environment, timeout=15)
        assert (completed.returncode == 0) == success, (name, completed.returncode, completed.stderr)
        assert source.read_text(encoding='utf-8') == content, name
        if success:
            transformed = output.read_text(encoding='utf-8')
            assert transformed.count('image: ${DEVICE_EDGE_IMAGE}') == 1, name
            before_lines, after_lines = content.splitlines(), transformed.splitlines()
            differences = [(a, b) for a, b in zip(before_lines, after_lines) if a != b]
            assert len(before_lines) == len(after_lines) and len(differences) <= 1, name
            assert 'image: emqx:1' in transformed and 'image: web:1' in transformed and 'image: unchanged' in transformed, name
            if '# keep' in content:
                assert '# keep' in transformed, name
        results.append({'case': name, 'passed': True, 'accepted': success})
    current, backup = root / 'current', root / 'snapshot'
    current.mkdir()
    backup.mkdir()
    (current / '.env').write_bytes(b'DEVICE_EDGE_IMAGE=edge:new\n')
    (current / 'docker-compose.yml').write_bytes(base.replace('edge:old', '${DEVICE_EDGE_IMAGE}').encode('utf-8'))
    (backup / '.env.before').write_bytes(b'DEVICE_EDGE_IMAGE=edge:old\n')
    (backup / 'docker-compose.yml').write_bytes(base.encode('utf-8'))
    calls = root / 'compose.calls'
    stub = 'event() { :; }\ncompose() { printf "%s\\n" "$*" >> "$COMPOSE_CALLS"; }\nrestore_image_reference() { :; }\nwait_for_service_state() { return 0; }\n'
    environment = dict(test_environment, COMPOSE_CALLS=calls.as_posix())
    subprocess.run([shell, '-c', stub + rollback + '\nSERVICE_NAME=device-edge\nrollback_service_upgrade "$1" "$2" edge:old sha256:old', 'fixture', current.as_posix(), (backup / '.env.before').as_posix()], env=environment, timeout=15, check=True)
    assert (current / '.env').read_bytes() == (backup / '.env.before').read_bytes()
    assert (current / 'docker-compose.yml').read_bytes() == (backup / 'docker-compose.yml').read_bytes()
    assert calls.read_text().strip() == '-f docker-compose.yml up -d --no-deps --force-recreate device-edge'
    results.append({'case': 'rollback_restores_env_and_compose_only_recreates_target', 'passed': True})
    old_id = 'sha256:' + ('1' * 64)
    new_id = 'sha256:' + ('2' * 64)
    tag_id_file = root / 'same-tag.id'
    running_id_file = root / 'same-running.id'
    tag_id_file.write_text(new_id, encoding='utf-8')
    running_id_file.write_text(old_id, encoding='utf-8')
    same_tag_stub = r'''
event() { :; }
reset_runtime_observation() { :; }
observe_runtime_service() {
  OBSERVATION_RESULT=ok
  OBSERVATION_FACTS="running||edge:stable|$(cat "$RUNNING_ID_FILE")"
}
compose() {
  case "$*" in
    '-f docker-compose.yml up -d --no-deps --force-recreate device-edge')
      cp "$TAG_ID_FILE" "$RUNNING_ID_FILE" ;;
    *) return 1 ;;
  esac
}
docker() {
  if [ "$1" = image ] && [ "$2" = inspect ] && [ "$3" = -f ]; then
    cat "$TAG_ID_FILE"
    return 0
  fi
  if [ "$1" = image ] && [ "$2" = tag ]; then
    printf '%s' "$3" > "$TAG_ID_FILE"
    return 0
  fi
  return 1
}
'''
    same_tag_environment = dict(
        test_environment,
        TAG_ID_FILE=tag_id_file.as_posix(),
        RUNNING_ID_FILE=running_id_file.as_posix(),
    )
    subprocess.run(
        [
            shell, '-c',
            same_tag_stub + load_image + verify_service + rollback
            + '\nSERVICE_NAME=device-edge\nrollback_service_upgrade "$1" "$2" edge:stable "$3"',
            'fixture', current.as_posix(), (backup / '.env.before').as_posix(), old_id,
        ],
        env=same_tag_environment, timeout=15, check=True,
    )
    assert tag_id_file.read_text() == old_id
    assert running_id_file.read_text() == old_id
    running_id_file.write_text(new_id, encoding='utf-8')
    rejected = subprocess.run(
        [
            shell, '-c',
            same_tag_stub + load_image + verify_service
            + '\nSERVICE_NAME=device-edge\nverify_service_state "$1" edge:stable "$2"',
            'fixture', current.as_posix(), old_id,
        ],
        env=same_tag_environment, timeout=15,
    )
    assert rejected.returncode != 0
    results.append({'case': 'same_tag_rollback_restores_and_verifies_previous_image_id', 'passed': True})
    health_stub = r'''
event() { :; }
sleep() { :; }
reset_runtime_observation() { :; }
observe_runtime_service() {
  OBSERVATION_RESULT=ok
  OBSERVATION_FACTS="$FIXTURE_FACTS"
}
'''
    for name, facts, success in [
        ('service_without_healthcheck_is_ready', f'running||edge:stable|{old_id}', True),
        ('service_healthy_is_ready', f'running|healthy|edge:stable|{old_id}', True),
        ('service_unhealthy_is_rejected', f'running|unhealthy|edge:stable|{old_id}', False),
    ]:
        completed = subprocess.run(
            [
                shell, '-c',
                health_stub + verify_service
                + '\nSERVICE_NAME=device-edge\nverify_service_state "$1" edge:stable "$2"',
                'fixture', current.as_posix(), old_id,
            ],
            env=dict(test_environment, FIXTURE_FACTS=facts), timeout=15,
        )
        assert (completed.returncode == 0) == success, name
        results.append({'case': name, 'passed': True})
    waiting_stub = f'''
event() {{ :; }}
sleep() {{ :; }}
reset_runtime_observation() {{ :; }}
attempts=0
observe_runtime_service() {{
  attempts=$((attempts + 1))
  OBSERVATION_RESULT=ok
  if [ "$attempts" -lt 3 ]; then
    OBSERVATION_FACTS="running|starting|edge:stable|{old_id}"
  else
    OBSERVATION_FACTS="running|healthy|edge:stable|{old_id}"
  fi
}}
'''
    subprocess.run(
        [
            shell, '-c',
            waiting_stub + verify_service
            + '\nSERVICE_NAME=device-edge\nwait_for_service_state "$1" edge:stable "$2"\n[ "$attempts" = 3 ]',
            'fixture', current.as_posix(), old_id,
        ],
        env=test_environment, timeout=15, check=True,
    )
    results.append({'case': 'service_starting_waits_until_healthy', 'passed': True})
    persistent_unhealthy = subprocess.run(
        [
            shell, '-c',
            health_stub + verify_service
            + '\nSERVICE_NAME=device-edge\nwait_for_service_state "$1" edge:stable "$2"',
            'fixture', current.as_posix(), old_id,
        ],
        env=dict(test_environment, FIXTURE_FACTS=f'running|unhealthy|edge:stable|{old_id}'),
        timeout=15,
    )
    assert persistent_unhealthy.returncode != 0
    results.append({'case': 'service_persistent_unhealthy_times_out', 'passed': True})
    full_health_stub = f'''
event() {{ :; }}
sleep() {{ :; }}
reset_runtime_observation() {{ :; }}
compose() {{ printf 'edge\n'; }}
attempts=0
observe_runtime_service() {{
  attempts=$((attempts + 1))
  OBSERVATION_RESULT=ok
  if [ "$attempts" -lt 3 ]; then
    OBSERVATION_FACTS="running|starting|edge:stable|{old_id}"
  else
    OBSERVATION_FACTS="running|healthy|edge:stable|{old_id}"
  fi
}}
'''
    subprocess.run(
        [shell, '-c', full_health_stub + verify_release_services + '\nwait_for_runtime_containers "$1"\n[ "$attempts" = 3 ]', 'fixture', current.as_posix()],
        env=test_environment, timeout=15, check=True,
    )
    results.append({'case': 'full_release_starting_waits_until_healthy', 'passed': True})
    full_deploy = root / 'full-deploy'
    full_release = full_deploy / 'releases' / 'old'
    full_new_release = full_deploy / 'releases' / 'new'
    full_snapshot = root / 'full-snapshot'
    full_release.mkdir(parents=True)
    full_new_release.mkdir()
    (full_release / 'docker-compose.yml').write_text('fixture', encoding='utf-8')
    (full_new_release / 'docker-compose.yml').write_text('fixture', encoding='utf-8')
    full_data = root / 'full-data'
    (full_data / 'config').mkdir(parents=True)
    (full_data / 'config' / 'host-info.json').write_text('new-host', encoding='utf-8')
    full_host_backup = root / 'full-host.before'
    full_host_backup.write_text('old-host', encoding='utf-8')
    full_ids = {
        'EDGE_TAG_ID': root / 'edge-tag.id',
        'WEB_TAG_ID': root / 'web-tag.id',
        'EDGE_RUNNING_ID': root / 'edge-running.id',
        'WEB_RUNNING_ID': root / 'web-running.id',
    }
    edge_old, web_old = 'sha256:' + ('3' * 64), 'sha256:' + ('4' * 64)
    for key, value in full_ids.items():
        value.write_text(edge_old if key.startswith('EDGE') else web_old, encoding='utf-8')
    full_snapshot_stub = r'''
event() { :; }
ln() { :; }
verify_runtime_containers() { :; }
wait_for_runtime_containers() { :; }
compose() {
  case "$*" in
    '-f docker-compose.yml config')
      printf 'services:\n  edge:\n    image: edge:stable\n  web:\n    image: web:stable\n' ;;
    '-f docker-compose.yml config --services') printf 'edge\nweb\n' ;;
    '-f docker-compose.yml ps -q edge') printf 'edge-container\n' ;;
    '-f docker-compose.yml ps -q web') printf 'web-container\n' ;;
    '-f docker-compose.yml up -d --force-recreate --remove-orphans')
      cp "$EDGE_TAG_ID" "$EDGE_RUNNING_ID"
      cp "$WEB_TAG_ID" "$WEB_RUNNING_ID" ;;
    '-f docker-compose.yml down --remove-orphans') : ;;
    *) return 1 ;;
  esac
}
docker() {
  if [ "$1" = image ] && [ "$2" = inspect ] && [ "$3" = -f ]; then
    case "$5" in
      edge:stable) cat "$EDGE_TAG_ID" ;;
      web:stable) cat "$WEB_TAG_ID" ;;
      *) return 1 ;;
    esac
    return 0
  fi
  if [ "$1" = image ] && [ "$2" = tag ]; then
    case "$4" in
      edge:stable) printf '%s' "$3" > "$EDGE_TAG_ID" ;;
      web:stable) printf '%s' "$3" > "$WEB_TAG_ID" ;;
      *) return 1 ;;
    esac
    return 0
  fi
  if [ "$1" = inspect ] && [ "$2" = -f ]; then
    case "$3:$4" in
      '{{.Image}}:edge-container') cat "$EDGE_RUNNING_ID" ;;
      '{{.Image}}:web-container') cat "$WEB_RUNNING_ID" ;;
      '{{.Config.Image}}:edge-container') printf 'edge:stable' ;;
      '{{.Config.Image}}:web-container') printf 'web:stable' ;;
      *) return 1 ;;
    esac
    return 0
  fi
  return 1
}
'''
    full_environment = dict(
        test_environment,
        DEPLOY_ROOT=full_deploy.as_posix(),
        DATA_ROOT=full_data.as_posix(),
        **{key: value.as_posix() for key, value in full_ids.items()},
    )
    subprocess.run(
        [
            shell, '-c',
            full_snapshot_stub + load_image + rollback_full
            + '\nsnapshot_release_images "$1" "$2"'
            + '\nprintf "%s" "$3" > "$EDGE_TAG_ID"'
            + '\nprintf "%s" "$4" > "$WEB_TAG_ID"'
            + '\nrollback_release "$5" "$1" "$6" true "$2"',
            'fixture', full_release.as_posix(), full_snapshot.as_posix(),
            'sha256:' + ('5' * 64), 'sha256:' + ('6' * 64),
            full_new_release.as_posix(), full_host_backup.as_posix(),
        ],
        env=full_environment, timeout=15, check=True,
    )
    assert full_ids['EDGE_TAG_ID'].read_text() == edge_old
    assert full_ids['WEB_TAG_ID'].read_text() == web_old
    assert full_ids['EDGE_RUNNING_ID'].read_text() == edge_old
    assert full_ids['WEB_RUNNING_ID'].read_text() == web_old
    assert not full_new_release.exists()
    assert (full_data / 'config' / 'host-info.json').read_text() == 'old-host'
    results.append({'case': 'full_release_rollback_restores_all_previous_image_ids', 'passed': True})
    install_cases = {
        'image_load_fail': (36, 'old-host', False),
        'stop_fail': (38, 'old-host', False),
        'host_publish_fail': (39, 'old-host', True),
        'compose_fail': (39, 'old-host', True),
        'health_fail': (41, 'old-host', True),
        'success': (0, 'new-host', False),
    }
    for case, (expected_exit, expected_host, old_release_restarted) in install_cases.items():
        case_root = root / ('install-' + case)
        data_root = case_root / 'data'
        deploy_root = case_root / 'deploy'
        old_release = deploy_root / 'releases' / 'old'
        release_source = case_root / 'release-source'
        for directory in [data_root / 'config', old_release, release_source / 'images']:
            directory.mkdir(parents=True)
        (data_root / 'config' / 'host-info.json').write_text('old-host', encoding='utf-8')
        (old_release / 'docker-compose.yml').write_text('old-compose', encoding='utf-8')
        (release_source / 'manifest.json').write_text('{"version":"fixture"}', encoding='utf-8')
        (release_source / 'images' / 'edge.tar').write_bytes(b'fixture')
        (release_source / 'images' / 'edge.tag').write_text('edge:new', encoding='utf-8')
        for name, content in [
            ('package.tar', 'package'), ('remote.env', 'env'),
            ('remote-compose.yml', 'compose'), ('remote-host.json', 'new-host'),
        ]:
            (case_root / name).write_text(content, encoding='utf-8')
        old_start_host = case_root / 'old-start-host'
        new_release = deploy_root / 'releases' / ('new-' + case)
        install_stub = r'''
event() { :; }
require_safe_release_version() { :; }
precheck() { :; }
need_cmd() { :; }
prepare_dirs() { mkdir -p "$DATA_ROOT/config" "$DEPLOY_ROOT/releases"; }
current_release_dir() { printf '%s' "$OLD_RELEASE"; }
find_release_src() { printf '%s' "$RELEASE_SOURCE"; }
snapshot_release_images() { mkdir -p "$2"; }
restore_release_image_snapshot() { :; }
verify_runtime_containers() { :; }
wait_for_runtime_containers() { :; }
verify_release_image_snapshot() { :; }
load_image_archive_with_tag() { [ "$CASE" != image_load_fail ]; }
stop_current_release() { [ "$CASE" != stop_fail ]; }
wait_for_release() { [ "$CASE" != health_fail ]; }
tar() { :; }
ln() { :; }
mv() {
  [ "$CASE" != host_publish_fail ] || return 1
  command mv "$@"
}
mktemp() {
  if [ "${1:-}" = -d ]; then
    path="$CASE_ROOT/tmp"
  else
    path="$DATA_ROOT/config/.host-info.json.candidate"
  fi
  mkdir -p "$path"
  if [ "${1:-}" != -d ]; then rmdir "$path"; : > "$path"; fi
  printf '%s' "$path"
}
compose() {
  case "$*" in
    '-f docker-compose.yml down --remove-orphans') return 0 ;;
    '-f docker-compose.yml up -d --force-recreate --remove-orphans')
      case "$PWD" in
        *"/new-$CASE") [ "$CASE" != compose_fail ] ;;
        *) cat "$DATA_ROOT/config/host-info.json" > "$OLD_START_HOST" ;;
      esac ;;
    *) return 1 ;;
  esac
}
fail() { exit "${3:-1}"; }
'''
        install_environment = dict(
            test_environment,
            CASE=case,
            CASE_ROOT=case_root.as_posix(),
            DATA_ROOT=data_root.as_posix(),
            DEPLOY_ROOT=deploy_root.as_posix(),
            OLD_RELEASE=old_release.as_posix(),
            NEW_RELEASE=new_release.as_posix(),
            RELEASE_SOURCE=release_source.as_posix(),
            OLD_START_HOST=old_start_host.as_posix(),
            RELEASE_VERSION='new-' + case,
            RELEASE_FINGERPRINT='',
            REMOTE_PACKAGE=(case_root / 'package.tar').as_posix(),
            REMOTE_ENV=(case_root / 'remote.env').as_posix(),
            REMOTE_COMPOSE=(case_root / 'remote-compose.yml').as_posix(),
            REMOTE_HOST_INFO=(case_root / 'remote-host.json').as_posix(),
        )
        install_script = (
            'set -eu\n' + load_image + '\n' + rollback_full + '\n' + install_full + '\n'
            + install_stub + '\ninstall_release'
        )
        syntax = subprocess.run(
            [shell, '-n'], input=install_script.encode('utf-8'),
            env=install_environment, capture_output=True, timeout=15,
        )
        assert syntax.returncode == 0, (
            syntax.stderr.decode('utf-8', errors='replace'),
            list(enumerate(install_script.splitlines(), start=1))[-30:],
        )
        completed = subprocess.run(
            [shell, '-s'], input=install_script.encode('utf-8'),
            env=install_environment, capture_output=True, timeout=15,
        )
        assert completed.returncode == expected_exit, (
            case, completed.returncode, completed.stderr.decode('utf-8', errors='replace')
            if isinstance(completed.stderr, bytes) else completed.stderr,
        )
        assert (data_root / 'config' / 'host-info.json').read_text() == expected_host, case
        assert not list((data_root / 'config').glob('.host-info.json.*')), case
        assert old_start_host.exists() == old_release_restarted, case
        if old_start_host.exists():
            assert old_start_host.read_text() == 'old-host', case
        results.append({'case': 'host_info_' + case, 'passed': True})
    archive = root / 'untagged.tar'
    archive.write_bytes(b'fixture')
    marker = root / 'docker-tag.call'
    image_id = 'sha256:' + ('a' * 64)
    docker_stub = r'''
docker() {
  if [ "$1" = load ] && [ "$2" = -i ]; then
    printf 'Loaded image ID: %s\n' "$FIXTURE_IMAGE_ID"
    if [ "${LOAD_MODE:-single}" = ambiguous ]; then
      printf 'Loaded image ID: sha256:%064d\n' 0
    fi
    return 0
  fi
  if [ "$1" = image ] && [ "$2" = inspect ]; then
    [ -f "$TAG_MARKER" ]
    return
  fi
  if [ "$1" = image ] && [ "$2" = tag ]; then
    printf '%s|%s' "$3" "$4" > "$TAG_MARKER"
    return 0
  fi
  return 1
}
'''
    load_environment = dict(test_environment, FIXTURE_IMAGE_ID=image_id, TAG_MARKER=marker.as_posix())
    subprocess.run(
        [shell, '-c', docker_stub + load_image + '\nload_image_archive_with_tag "$1" repo/manual:1', 'fixture', archive.as_posix()],
        env=load_environment, timeout=15, check=True,
    )
    assert marker.read_text() == image_id + '|repo/manual:1'
    results.append({'case': 'untagged_image_is_assigned_manual_tag', 'passed': True})
    marker.write_text('preexisting', encoding='utf-8')
    subprocess.run(
        [shell, '-c', docker_stub + load_image + '\nload_image_archive_with_tag "$1" repo/manual:1', 'fixture', archive.as_posix()],
        env=load_environment, timeout=15, check=True,
    )
    assert marker.read_text() == image_id + '|repo/manual:1'
    results.append({'case': 'untagged_image_replaces_existing_target_tag', 'passed': True})
    marker.unlink()
    ambiguous_environment = dict(load_environment, LOAD_MODE='ambiguous')
    ambiguous = subprocess.run(
        [shell, '-c', docker_stub + load_image + '\nload_image_archive_with_tag "$1" repo/manual:1', 'fixture', archive.as_posix()],
        env=ambiguous_environment, timeout=15,
    )
    assert ambiguous.returncode != 0 and not marker.exists()
    results.append({'case': 'multiple_untagged_image_ids_are_rejected', 'passed': True})
result = {'agentSha256': hashlib.sha256(agent.encode('utf-8')).hexdigest(), 'runtime': shell, 'tests': results, 'passed': len(results), 'realDockerAccessed': False}
if arguments.output:
    with Path(arguments.output).open('x', encoding='utf-8') as evidence_file:
        json.dump(result, evidence_file, ensure_ascii=False, indent=2)
print(json.dumps(result, indent=2))
