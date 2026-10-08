"""在授权的一体机上只读采集服务身份，不输出环境变量或连接凭据。"""
import datetime
import json
import os
import subprocess


def run(*args):
    return subprocess.check_output(args, universal_newlines=True).strip()


ips = run('hostname', '-I').split()
assert any(ip in ips for ip in ['192.168.3.79', '192.168.3.121'])
ids = run('docker', 'ps', '-aq', '--no-trunc').split()
containers = []
for item in json.loads(run('docker', 'inspect', *ids)) if ids else []:
    labels = item['Config'].get('Labels') or {}
    containers.append({
        'id': item['Id'], 'name': item['Name'].lstrip('/'),
        'imageId': item['Image'], 'imageTag': item['Config']['Image'],
        'status': item['State']['Status'], 'startedAt': item['State']['StartedAt'],
        'restartCount': item.get('RestartCount'),
        'composeService': labels.get('com.docker.compose.service'),
    })
with open('/opt/data/config/host-info.json') as source:
    config = json.load(source)
print(json.dumps({
    'timeUtc': datetime.datetime.utcnow().isoformat() + 'Z',
    'ips': ips, 'containers': containers,
    'hostInfoIdentity': {key: config.get(key) for key in ['mac', 'ip', 'hostname']},
    'currentRelease': os.path.realpath('/opt/data/deploy/current'),
}, indent=2))
