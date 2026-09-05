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
load_image = agent[agent.index('load_image_archive_with_tag() {'):agent.index('\nfind_release_src() {')]
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
with tempfile.TemporaryDirectory(prefix='inxaiot-compose-contract-') as scratch:
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
    stub = 'event() { :; }\ncompose() { printf "%s\\n" "$*" >> "$COMPOSE_CALLS"; }\nverify_service_state() { return 0; }\n'
    environment = dict(test_environment, COMPOSE_CALLS=calls.as_posix())
    subprocess.run([shell, '-c', stub + rollback + '\nSERVICE_NAME=device-edge\nrollback_service_upgrade "$1" "$2" edge:old', 'fixture', current.as_posix(), (backup / '.env.before').as_posix()], env=environment, timeout=15, check=True)
    assert (current / '.env').read_bytes() == (backup / '.env.before').read_bytes()
    assert (current / 'docker-compose.yml').read_bytes() == (backup / 'docker-compose.yml').read_bytes()
    assert calls.read_text().strip() == '-f docker-compose.yml up -d --no-deps --force-recreate device-edge'
    results.append({'case': 'rollback_restores_env_and_compose_only_recreates_target', 'passed': True})
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
