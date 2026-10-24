"""第二步 Agent 镜像归档清理实机验收：只用于79/121独立目录和无网络测试容器。

正式工作台整包/单服链由 Rust harness 验证；此脚本覆盖 Agent 首次安装和失败回滚。
必须等第一步通过、Agent 修改完成后，提供预期版本与 SHA 才能执行。
"""
from __future__ import annotations
import argparse, hashlib, io, json, os, pathlib, re, subprocess, tarfile, tempfile, time, uuid

ROOT = pathlib.Path(__file__).resolve().parents[2]
HOSTS = ("192.168.3.79", "192.168.3.121")
IMAGES = {
    "web": ("device-edge-web-1.0.1.Alpha.20260812.tar", "192.168.3.48:5000/inxvision/device-edge-web:1.0.1.Alpha.20260812"),
    "rule": ("rule-engine-1.0.0.Alpha.20260814.tar", "192.168.3.48:5000/inxvision/rule-engine:1.0.0.Alpha.20260814"),
}
BASELINE = r'''import json,os,subprocess
names=['inx-device-edge','inx-rule-engine','inx-device-edge-web','inx-edge-emqx']
rows=json.loads(subprocess.check_output(['docker','inspect']+names))
print(json.dumps({'current':os.path.realpath('/opt/data/deploy/current'),'containers':sorted([{'name':r['Name'],'id':r['Id'],'image':r['Image'],'running':r['State']['Running'],'started':r['State']['StartedAt']}for r in rows],key=lambda r:r['name'])}))
'''
REMOTE = r'''import json,os,re,subprocess,sys,hashlib,glob,shutil
x=json.load(sys.stdin);root=x['root'];token=x['token'];action=x['action']
assert re.fullmatch(r'/opt/data/inx-artifact-agent-[a-f0-9]{32}',root) and root.endswith(token)
assert os.path.realpath(root)==root
def run(args,**kw):return subprocess.run(args,stdout=subprocess.PIPE,stderr=subprocess.PIPE,universal_newlines=True,**kw)
def digest(p):return hashlib.sha256(open(p,'rb').read()).hexdigest()
if action=='prepare':
 assert not os.path.lexists(root);os.mkdir(root);open(root+'/.scope','w').write(token)
 os.makedirs(root+'/inputs');print(json.dumps({'prepared':True}));sys.exit(0)
assert open(root+'/.scope').read()==token
if action=='files':
 for path,body in x['files'].items():
  assert re.fullmatch(r'[a-zA-Z0-9_.-]+',path)
  open(root+'/inputs/'+path,'w').write(body)
 print(json.dumps({'written':len(x['files'])}));sys.exit(0)
if action=='invoke':
 agent=root+'/agent.sh';assert digest(agent)==x['sha']
 env=os.environ.copy();env.update(x['env'])
 assert env['DATA_ROOT']==root+'/data' and env['DEPLOY_ROOT']==root+'/deploy'
 for key in ['REMOTE_PACKAGE','REMOTE_IMAGE','REMOTE_ENV','REMOTE_COMPOSE','REMOTE_HOST_INFO','COMPOSE_CMD']:
  if env.get(key):assert env[key].startswith(root+'/')
 p=run(['sh',agent,x['agentAction']],env=env,timeout=240)
 print(json.dumps({'returncode':p.returncode,'stdout':p.stdout,'stderr':p.stderr}));sys.exit(0)
if action=='facts':
 current=os.path.realpath(root+'/deploy/current');assert current.startswith(root+'/deploy/releases/')
 p=run(['docker','inspect','inx-agent-cleanup-'+token]);assert p.returncode==0
 c=json.loads(p.stdout)[0]
 release_tars={os.path.relpath(p,root):os.path.getsize(p)for p in glob.glob(root+'/deploy/releases/*/images/*.tar')}
 service_tars={os.path.relpath(p,root):os.path.getsize(p)for p in glob.glob(root+'/deploy/service-upgrades/*/*.tar')}
 print(json.dumps({'current':current,'containerId':c['Id'],'imageId':c['Image'],'imageTag':c['Config']['Image'],'running':c['State']['Running'],'health':c['State'].get('Health',{}).get('Status'),'envSha':digest(current+'/.env'),'composeSha':digest(current+'/docker-compose.yml'),'metadataExists':all(os.path.isfile(current+'/'+n)for n in ['.env','docker-compose.yml','manifest.json','checksums/sha256.txt','images/cleanup-probe.tag']),'releaseTars':release_tars,'serviceTars':service_tars,'backupDirs':glob.glob(root+'/data/backup/*'),'serviceMetadata':glob.glob(root+'/deploy/service-upgrades/*/metadata.json')}));sys.exit(0)
if action=='cleanup':
 name='inx-agent-cleanup-'+token
 p=run(['docker','ps','-aq','--filter','name=^'+name+'$'])
 if p.stdout.strip():
  c=json.loads(subprocess.check_output(['docker','inspect',name]))[0]
  assert c['Name']=='/'+name and c['Config']['Labels']['com.docker.compose.project']=='inxagent'+token
  r=run(['docker','rm','-f',name]);assert r.returncode==0
 assert not subprocess.check_output(['docker','ps','-aq','--filter','name=^'+name+'$']).strip()
 shutil.rmtree(root);print(json.dumps({'removed':True}));sys.exit(0)
raise RuntimeError('unknown action')
'''

def digest(path):
    with open(path, 'rb') as file:
        return hashlib.file_digest(file, 'sha256').hexdigest()

def source_identity(path, tag):
    with tarfile.open(path) as archive:
        manifest = json.load(archive.extractfile('manifest.json'))
        assert len(manifest) == 1 and manifest[0]['RepoTags'] == [tag]
        config = archive.extractfile(manifest[0]['Config']).read()
        return 'sha256:' + hashlib.sha256(config).hexdigest()

def compose(token, healthy=True):
    return f'''name: inxagent{token}
services:
  cleanup-probe:
    image: ${{PROBE_IMAGE}}
    container_name: inx-agent-cleanup-{token}
    entrypoint: ["/bin/sh", "-c", "while :; do sleep 10; done"]
    network_mode: none
    healthcheck:
      test: ["CMD", "/bin/sh", "-c", "{'true' if healthy else 'false'}"]
      interval: 1s
      timeout: 1s
      retries: 2
'''

def bundle(output, version, source, tag, invalid=False):
    image_bytes = b'fixture-invalid-docker-image' if invalid else None
    image_sha = hashlib.sha256(image_bytes).hexdigest() if invalid else digest(source)
    manifest = {'schemaVersion': 1, 'version': version, 'images': [{'service':'cleanup-probe','image':tag,'archive':'images/cleanup-probe.tar','sha256':image_sha}]}
    tag_bytes = (tag+'\n').encode()
    texts = {'manifest.json':json.dumps(manifest,indent=2).encode(), 'images/cleanup-probe.tag':tag_bytes,
             'checksums/sha256.txt':f'{image_sha}  images/cleanup-probe.tar\n{hashlib.sha256(tag_bytes).hexdigest()}  images/cleanup-probe.tag\n'.encode()}
    with tarfile.open(output,'w') as archive:
        if invalid:
            info=tarfile.TarInfo('images/cleanup-probe.tar');info.size=len(image_bytes);archive.addfile(info,io.BytesIO(image_bytes))
        else:archive.add(source,arcname='images/cleanup-probe.tar',recursive=False)
        for name,data in texts.items():
            info=tarfile.TarInfo(name);info.size=len(data);archive.addfile(info,io.BytesIO(data))

def main():
    parser=argparse.ArgumentParser();parser.add_argument('--expected-version',required=True);parser.add_argument('--expected-sha',required=True);args=parser.parse_args()
    assert os.environ.get('INX_AGENT_TAR_CLEANUP_REAL')=='YES', '需要第二步明确测试门禁'
    assert re.fullmatch('[0-9a-f]{64}',args.expected_sha)
    agent=ROOT/'src-tauri/resources/agent/edge-node-agent.sh'
    assert digest(agent)==args.expected_sha and f'AGENT_VERSION="{args.expected_version}"' in agent.read_text(encoding='utf-8')
    key=pathlib.Path(os.environ['INX_TEST_SSH_PRIVATE_KEY_PATH']).resolve();assert key.is_file()
    description=(ROOT/'test/测试数据说明.txt').read_text(encoding='utf-8-sig')
    line=lambda label:next(v.split(label,1)[1].strip()for v in description.splitlines()if v.strip().startswith(label))
    assert tuple(line('一体机IP:').split('/'))==HOSTS
    assert line('平台主机：')=='192.168.3.142'
    api_host,api_port=line('平台API：').split(':');mqtt_host,mqtt_port=line('平台mqtt：').split(':')
    user=line('一体机ssh用户：');assert user=='root'
    token=uuid.uuid4().hex;remote_root='/opt/data/inx-artifact-agent-'+token
    directory=ROOT/'.review-tools/artifact-cleanup-20261024'/('agent-'+token);directory.mkdir()
    report={'passed':False,'token':token,'remoteRoot':remote_root,'agentVersion':args.expected_version,'agentSha256':args.expected_sha,'hosts':{},'productionDataUsed':False}
    def persist():
        (directory/'result.json').write_text(json.dumps(report,ensure_ascii=False,indent=2),encoding='utf-8')
    persist()
    def ssh(host,code,payload=None):
        code=('import io,sys\nsys.stdin=io.StringIO('+repr(json.dumps(payload))+')\n'+code) if payload is not None else code
        p=subprocess.run(['ssh','-i',str(key),'-o','BatchMode=yes','-o','StrictHostKeyChecking=yes','-o','ConnectTimeout=10',user+'@'+host,'python3 -'],input=code,text=True,capture_output=True,timeout=280)
        assert p.returncode==0, 'SSH测试命令失败，保留独立现场'
        return json.loads(p.stdout)
    def remote(host,action,**kw):return ssh(host,REMOTE,{'root':remote_root,'token':token,'action':action,**kw})
    def upload(host,local,name):
        assert re.fullmatch('[a-zA-Z0-9_.-]+',name)
        p=subprocess.run(['scp','-i',str(key),'-o','BatchMode=yes','-o','StrictHostKeyChecking=yes',str(local),user+'@'+host+':'+remote_root+'/'+name],capture_output=True,text=True,timeout=240)
        assert p.returncode==0, '测试文件上传失败'
    sources={k:ROOT/'test/images'/v[0] for k,v in IMAGES.items()}
    ids={k:source_identity(sources[k],v[1])for k,v in IMAGES.items()}
    hashes={k:digest(v)for k,v in sources.items()}
    report['sourceImageIds']=ids;report['sourceHashes']=hashes;persist()
    with tempfile.TemporaryDirectory(prefix='inx-agent-cleanup-bundles-') as temp:
        temp=pathlib.Path(temp)
        for name,key_name,invalid in [('first','web',False),('full','rule',False),('bad-load','web',True),('bad-health','rule',False)]:
            bundle(temp/(name+'.tar'),name,sources[key_name],IMAGES[key_name][1],invalid)
        upload_bytes=sum(p.stat().st_size for p in temp.glob('*.tar'))+sum(p.stat().st_size for p in sources.values())+agent.stat().st_size
        required_free_bytes=upload_bytes*2+1024**3
        for host in HOSTS:
            before=ssh(host,BASELINE);assert all(c['running']for c in before['containers'])
            for key_name,(_,tag)in IMAGES.items():
                code='import json,subprocess\nprint(json.dumps(subprocess.check_output('+repr(['docker','image','inspect','--format','{{.Id}}',tag])+').decode().strip()))'
                assert ssh(host,code)==ids[key_name], '源镜像与既有镜像ID不同，禁止覆盖标签'
            space=ssh(host,"import os,json\ns=os.statvfs('/opt/data')\nprint(json.dumps({'freeBytes':s.f_bavail*s.f_frsize}))")
            row={'baseline':before,'restoreRequired':False,'runs':[],
                 'spaceGate':{'filesystemPath':'/opt/data','freeBytes':space['freeBytes'],'uploadBytes':upload_bytes,'requiredFreeBytes':required_free_bytes,'passed':space['freeBytes']>=required_free_bytes}}
            report['hosts'][host]=row;persist()
            assert row['spaceGate']['passed'], '测试设备剩余空间不足：需两倍待上传文件体积加1GiB余量，尚未创建目录或上传'
            row['restoreRequired']=True;persist()
            remote(host,'prepare');upload(host,agent,'agent.sh')
            for source in temp.glob('*.tar'):upload(host,source,source.name)
            upload(host,sources['web'],'service-web.tar');upload(host,sources['rule'],'service-rule.tar')
            env_base={'DATA_ROOT':remote_root+'/data','DEPLOY_ROOT':remote_root+'/deploy','PLATFORM_API_HOST':api_host,'PLATFORM_API_PORT':api_port,'PLATFORM_MQTT_HOST':mqtt_host,'PLATFORM_MQTT_PORT':mqtt_port,'TASK_ID':token,'SERVICE_NAME':'cleanup-probe','SERVICE_IMAGE_ENV':'PROBE_IMAGE'}
            def invoke(label,action,env,expected_success):
                result=remote(host,'invoke',sha=args.expected_sha,agentAction=action,env=env_base|env)
                (directory/(host+'-'+label+'.log')).write_text(result['stdout']+'\n'+result['stderr'],encoding='utf-8')
                assert (result['returncode']==0)==expected_success, label+'退出状态不符'
                facts=remote(host,'facts');row['runs'].append({'case':label,'returncode':result['returncode'],'facts':facts});persist()
                assert facts['metadataExists'] and facts['running'] and facts['health']=='healthy'
                assert ssh(host,BASELINE)==before, '既有业务容器或current发生变化'
                return facts,result
            def release_env(name,image_key,healthy=True):
                remote(host,'files',files={name+'.env':'RELEASE_VERSION='+name+'\nPROBE_IMAGE='+IMAGES[image_key][1]+'\n',name+'.compose.yml':compose(token,healthy),name+'.host-info.json':json.dumps({'fixture':token})})
                return {'RELEASE_VERSION':name,'REMOTE_PACKAGE':remote_root+'/'+name+'.tar','REMOTE_ENV':remote_root+'/inputs/'+name+'.env','REMOTE_COMPOSE':remote_root+'/inputs/'+name+'.compose.yml','REMOTE_HOST_INFO':remote_root+'/inputs/'+name+'.host-info.json'}
            facts,_=invoke('first','install',release_env('first','web'),True);assert not facts['releaseTars']
            backup=remote(host,'invoke',sha=args.expected_sha,agentAction='backup',env=env_base|{'RELEASE_VERSION':'full'});assert backup['returncode']==0
            facts,_=invoke('full','install',release_env('full','rule'),True);assert not facts['releaseTars'] and facts['backupDirs']
            facts,_=invoke('service','service-upgrade',{'SERVICE_IMAGE':IMAGES['web'][1],'REMOTE_IMAGE':remote_root+'/service-web.tar'},True);assert not facts['serviceTars']
            stable=facts
            facts,_=invoke('bad-load','install',release_env('bad-load','web'),False)
            assert any('/bad-load/images/'in p for p in facts['releaseTars']) and facts['current']==stable['current'] and facts['containerId']==stable['containerId']
            facts,log=invoke('bad-health','install',release_env('bad-health','rule',False),False)
            assert 'previous release restored' in log['stdout']
            for k in ['current','imageId','envSha','composeSha']:assert facts[k]==stable[k]
            shim=f'''#!/bin/sh
set -eu
case " $* " in
  *" up "*)
    if [ ! -f "{remote_root}/fault-used" ]; then
      : > "{remote_root}/fault-used"
      docker compose "$@"
      docker stop "inx-agent-cleanup-{token}" >/dev/null
      exit 0
    fi
    ;;
esac
exec docker compose "$@"
'''
            remote(host,'files',files={'compose-fault.sh':shim})
            # COMPOSE_CMD接受命令字符串，通过sh执行限定在本次marker目录内的shim。
            # invoke路径门禁只接受目录路径，故单独用固定脚本文件的可执行位。
            ssh(host,"import os,json\np="+repr(remote_root+'/inputs/compose-fault.sh')+"\nos.chmod(p,0o700)\nprint(json.dumps(True))")
            facts,log=invoke('service-health-fail','service-upgrade',{'SERVICE_IMAGE':IMAGES['rule'][1],'REMOTE_IMAGE':remote_root+'/service-rule.tar','COMPOSE_CMD':remote_root+'/inputs/compose-fault.sh'},False)
            assert 'previous service image restored' in log['stdout'] and facts['serviceTars'] and facts['serviceMetadata']
            for k in ['current','imageId','envSha','composeSha']:assert facts[k]==stable[k]
            remote(host,'cleanup');row['restoreRequired']=False;row['removed']=True
            row['finalBaseline']=ssh(host,BASELINE);assert row['finalBaseline']==before;persist()
    assert {k:digest(v)for k,v in sources.items()}==hashes
    report['passed']=True;report['sourceImagesPreserved']=True;persist()
    print(directory/'result.json')

if __name__=='__main__':main()
