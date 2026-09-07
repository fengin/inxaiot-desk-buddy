"""只在临时目录执行提取的只读采集函数；Compose和Docker完全由函数桩替代。"""
import argparse
import json
import os
from pathlib import Path
import subprocess
import tempfile

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--shell', required=True)
parser.add_argument('--cached-only', action='store_true', help='只执行观测复用与失败出口回归')
arguments = parser.parse_args()
agent = (Path(__file__).resolve().parents[2] / 'src-tauri/resources/agent/edge-node-agent.sh').read_text(encoding='utf-8')
functions = agent[agent.index('configured_service_image() {'):agent.index('\nservice_health() {')]
escape = agent[agent.index('json_escape() {'):agent.index('\nevent() {')]
stub = r'''
current_release_dir() { [ "$CASE" = no_current ] || printf '%s' "$FIXTURE_DIR"; }
compose() {
  if [ -n "${CALLS_FILE:-}" ]; then printf 'compose %s\n' "$*" >> "$CALLS_FILE"; fi
  case "$*" in
    '-f docker-compose.yml config --services') printf 'edge\nrule\nweb\n' ;;
    '-f docker-compose.yml config')
      [ "$CASE" != bad_compose ] || return 1
      printf 'services:\n  edge:\n    image: repo/edge:2\n  rule:\n    image: "repo/rule:2"\n  web:\n    image: repo/web:2\n'
      ;;
    '-f docker-compose.yml ps -a -q edge'|'-f docker-compose.yml ps -q edge') [ "$CASE" = missing ] || printf 'edge-id\n' ;;
    '-f docker-compose.yml ps -a -q rule'|'-f docker-compose.yml ps -q rule') printf 'rule-id\n' ;;
    '-f docker-compose.yml ps -a -q web'|'-f docker-compose.yml ps -q web') printf 'web-id\n' ;;
    *) printf '禁止的Compose命令:%s' "$*" >&2; exit 99 ;;
  esac
}
docker() {
  if [ -n "${CALLS_FILE:-}" ]; then printf 'docker %s\n' "$4" >> "$CALLS_FILE"; fi
  [ "$1" = inspect ] || exit 98
  [ "$CASE" != inspect_error ] || return 1
  case "$4" in
    edge-id)
      case "$CASE" in
        unhealthy) printf 'running|unhealthy|repo/edge:2|sha256:edge-real\n' ;;
        mismatch) printf 'running||repo/edge:1|sha256:edge-old\n' ;;
        stopped) printf 'exited||repo/edge:2|sha256:edge-real\n' ;;
        *) printf 'running||repo/edge:2|sha256:edge-real\n' ;;
      esac ;;
    rule-id) printf 'running|healthy|repo/rule:2|sha256:rule-real\n' ;;
    web-id) printf 'running||repo/web:2|sha256:web-real\n' ;;
    *) exit 97 ;;
  esac
}
'''
environment = dict(os.environ)
environment['PATH'] = str(Path(arguments.shell).parent) + os.pathsep + environment.get('PATH', '')
subprocess.run([arguments.shell, '-n'], input=agent.encode('utf-8'), env=environment, timeout=15, check=True)
results = []
with tempfile.TemporaryDirectory(prefix='inxaiot-observation-') as scratch:
    root = Path(scratch)
    compose = root / 'docker-compose.yml'
    compose.write_bytes(b'fixture-compose-unchanged')
    sources = [] if arguments.cached_only else [('first_deploy', ''), ('full_upgrade', ''), ('service_upgrade', 'edge'), ('manual', ''), ('rollback', 'edge')]
    for source, service in sources:
        for case in ['normal', 'unhealthy', 'mismatch', 'stopped', 'missing', 'inspect_error', 'no_current', 'bad_compose']:
            env = dict(environment, CASE=case, FIXTURE_DIR=root.as_posix(), SERVICE_CHECK_SOURCE=source, SERVICE_NAME=service)
            completed = subprocess.run([arguments.shell, '-s'], input=('set -eu\n' + escape + stub + functions + '\ninspect_services').encode('utf-8'), env=env, capture_output=True, timeout=15)
            assert completed.returncode == 0, (case, completed.stderr.decode('utf-8'))
            report = json.loads(completed.stdout)['report']
            assert report['source'] == source
            assert report['scope'] == ('service' if service else 'all')
            assert report['checkedAt'].endswith('Z') and report['startedAt'].endswith('Z')
            if case in ['no_current', 'bad_compose']:
                assert report['state'] == 'failed' and not report['services'] and report['error']
            else:
                assert report['state'] == ('failed' if case == 'inspect_error' else 'succeeded')
                assert report['expectedServices'] == ['edge', 'rule', 'web']
                assert [item['serviceName'] for item in report['services']] == (['edge'] if service else ['edge', 'rule', 'web'])
                observed = report['services'][0]
                assert observed['state'] == {'normal': 'normal', 'unhealthy': 'abnormal', 'mismatch': 'version_mismatch', 'stopped': 'abnormal', 'missing': 'abnormal', 'inspect_error': 'unknown'}[case]
                if case == 'normal':
                    assert observed['actualImage'] == 'repo/edge:2' and observed['imageId'] == 'sha256:edge-real'
                if case == 'mismatch':
                    assert observed['actualImage'] != observed['expectedImage']
                if case == 'unhealthy':
                    assert observed['healthStatus'] == 'unhealthy'
            assert compose.read_bytes() == b'fixture-compose-unchanged'
            assert list(root.iterdir()) == [compose]
            results.append({'source': source, 'case': case, 'passed': True})
    failure_functions = agent[agent.index('event() {'):agent.index('\nrequire_safe_release_version() {')]
    env = dict(environment, CASE='normal', FIXTURE_DIR=root.as_posix(), SERVICE_CHECK_SOURCE='service_upgrade', SERVICE_NAME='edge', ACTION='service-upgrade', ROLLBACK_OBSERVED='true')
    failed = subprocess.run([arguments.shell, '-s'], input=('set -eu\n' + escape + stub + functions + '\n' + failure_functions + '\nfail service_upgrade "service health failed; previous image restored" 86').encode('utf-8'), env=env, capture_output=True, timeout=15)
    assert failed.returncode == 86
    events = [json.loads(line) for line in failed.stdout.splitlines()]
    assert events[0]['status'] == 'failed'
    assert events[-1]['report']['source'] == 'rollback'
    assert events[-1]['report']['services'][0]['actualImage'] == 'repo/edge:2'
    results.append({'case': 'rollback_observation_keeps_original_failure_exit_status', 'passed': True})
    verify_all = agent[agent.index('verify_runtime_containers() {'):agent.index('\nrollback_release() {')]
    verify_one = agent[agent.index('verify_service_state() {'):agent.index('\n# 只兼容转换目标服务')]
    for mode, verifier, service in [('all', 'verify_runtime_containers "$FIXTURE_DIR"', ''), ('service', 'verify_service_state "$FIXTURE_DIR" repo/edge:2', 'edge')]:
        for case in ['normal', 'unhealthy', 'mismatch']:
            calls = root / f'{mode}-{case}.calls'
            env = dict(environment, CASE=case, FIXTURE_DIR=root.as_posix(), SERVICE_CHECK_SOURCE='manual', SERVICE_NAME=service, CALLS_FILE=calls.as_posix())
            script = 'set -eu\n' + escape + stub + functions + '\n' + verify_all + verify_one + '\nverify_result=0\n' + verifier + ' || verify_result=$?\ninspect_services\nexit "$verify_result"\n'
            checked = subprocess.run([arguments.shell, '-s'], input=script.encode('utf-8'), env=env, capture_output=True, timeout=15)
            expected_exit = 1 if case == 'unhealthy' or (mode == 'service' and case == 'mismatch') else 0
            assert checked.returncode == expected_exit, (mode, case, checked.stderr.decode('utf-8'))
            report = json.loads(checked.stdout)['report']
            assert report['services'][0]['state'] == {'normal': 'normal', 'unhealthy': 'abnormal', 'mismatch': 'version_mismatch'}[case]
            call_lines = calls.read_text(encoding='utf-8').splitlines()
            for name in (['edge'] if service else ['edge', 'rule', 'web']):
                assert call_lines.count(f'docker {name}-id') == 1, (mode, case, call_lines)
                assert call_lines.count(f'compose -f docker-compose.yml ps -q {name}') == 1, (mode, case, name, call_lines)
                assert call_lines.count(f'compose -f docker-compose.yml ps -a -q {name}') == 0
            assert call_lines.count('compose -f docker-compose.yml config --services') == 1
            results.append({'case': f'{mode}_{case}_verification_reuses_same_observation', 'passed': True})
print(json.dumps({'passed': len(results), 'realDockerAccessed': False, 'remoteHostAccessed': False, 'tests': results}, ensure_ascii=False))
