"""Independent q32 public artifact/runtime acceptance; build-test only, no production/inference."""
from pathlib import Path
import argparse,datetime,hashlib,importlib.util,json,os,platform,re,socket,struct,subprocess,sys,tarfile,tempfile,time

def sha(path):
 h=hashlib.sha256()
 with Path(path).open('rb') as f:
  for block in iter(lambda:f.read(1024*1024),b''):h.update(block)
 return h.hexdigest()
def load(name,path):
 spec=importlib.util.spec_from_file_location(name,path);module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module);return module
p=argparse.ArgumentParser();p.add_argument('--commit',required=True);p.add_argument('--release',type=Path,required=True);p.add_argument('--source-hashes',type=Path,required=True);p.add_argument('--source',type=Path,required=True);p.add_argument('--assets',type=Path,required=True);p.add_argument('--isolated-qa',type=Path,required=True);p.add_argument('--ci-url',required=True);p.add_argument('--out',type=Path,required=True);a=p.parse_args()
assert socket.gethostname()=='build-test' and platform.system()=='Linux' and platform.machine()=='x86_64';assert re.fullmatch('[0-9a-f]{40}',a.commit)
a.out.mkdir(parents=True,exist_ok=False);started=time.monotonic();record=dict(host=socket.gethostname(),source_commit=a.commit,tag='v0.34.0-qiwolf.32',exact_main_ci_url=a.ci_url,production_access=False,inference_calls=0,started_utc=datetime.datetime.now(datetime.timezone.utc).isoformat())
def save(name,value):(a.out/name).write_text(json.dumps(value,indent=2)+'\n')
def run(command,name,env=None,timeout=1200):
 with (a.out/(name+'.log')).open('w') as log:r=subprocess.run(command,stdout=log,stderr=subprocess.STDOUT,env=env,timeout=timeout)
 save(name+'-command.json',dict(command=list(map(str,command)),exit_code=r.returncode));assert r.returncode==0,name+' failed; inspect isolated log'
try:
 release=json.loads(a.release.read_text());binding=json.loads(a.source_hashes.read_text());assert binding['commit']==a.commit
 assert release['tag_name']==record['tag'] and release['draft'] is False and release['prerelease'] is False
 assets=release['assets'];expected={f'tokengo-{a.commit[:12]}-linux-x86_64.tar.gz',f'tokengo-{a.commit[:12]}-linux-x86_64.tar.gz.sha256',f'tokengo-{a.commit}-linux-amd64.tar.gz','compose.yaml','container.env.example','container-deployment.md','image-manifest.json','image-SHA256SUMS','registry-image.txt'}
 assert len(assets)==9 and {r['name'] for r in assets}==expected
 official=[]
 for row in assets:
  path=a.assets/row['name'];assert path.is_file() and not path.is_symlink();assert path.stat().st_size==row['size'] and 'sha256:'+sha(path)==row['digest'];official.append(dict(name=row['name'],size=row['size'],digest=row['digest']))
 save('official-assets.json',dict(result='passed',anonymous_download_receipt='../download-receipt.json',assets=official))
 manager_path=a.source/'deploy/manage.py';assert sha(manager_path)==binding['files']['deploy/manage.py'];manager=load('q32_source_manager',manager_path)
 verified=load('q32_verify_assets',Path(__file__).parent/'helpers/verify-assets.py').verify(a.assets,a.commit,manager_path);save('archive-checksums.json',verified)
 native=a.assets/verified['native_archive'];native_sha=sha(native)
 with tempfile.TemporaryDirectory(prefix='q32-package-readonly-') as temp:
  tree,commit=manager.verify_bundle(native,Path(temp));assert commit==a.commit
  manifest=json.loads((tree/'manifest.json').read_text());assert manifest['schema_version']==47 and manifest['target']=='x86_64-unknown-linux-gnu';assert manifest['cargo_lock_sha256']==binding['files']['Cargo.lock']
  documents={name:value for name,value in binding['files'].items() if name.startswith('docs/')};packaged={p.relative_to(tree).as_posix() for p in (tree/'docs').rglob('*') if p.is_file()};assert packaged==set(documents),'Packaged docs differ from exact tag inventory'
  for name,value in documents.items():assert sha(tree/name)==value,name
  for name in ['deployment.md','operations-tokens.md','account-login.md']:assert sha(tree/name)==documents['docs/'+name]
  assert (tree/'docs/container-deployment.md').read_bytes()==(a.assets/'container-deployment.md').read_bytes()
  save('packaged-documents.json',dict(result='passed',source_commit=a.commit,count=len(documents),documents=documents,standalone_doc_matches=True,untracked_files_absent=True))
  for version in ['43','44','46','47']:
   row=manifest['migrations'][version];assert sha(tree/row['archive_path'])==row['sha256']==binding['files'][row['source_path']]
  row=manifest['migrations']['45'];parts=[]
  for part in row['parts']:
   raw=(tree/part['archive_path']).read_bytes();assert hashlib.sha256(raw).hexdigest()==part['sha256']==binding['files'][part['source_path']];parts.append(raw)
  assert hashlib.sha256(b'\n'.join(parts)).hexdigest()==row['sha256']==sha(tree/row['archive_path']);assert set(manifest['migrations'])=={'43','44','45','46','47'}
  abi={}
  for name in ['tokengo','tokengo-server']:
   binary=tree/'bin'/name
   with binary.open('rb') as stream:header=stream.read(64)
   assert header[:6]==b'\x7fELF\x02\x01' and struct.unpack_from('<H',header,18)[0]==62
   details=subprocess.check_output(['readelf','-h','-l',str(binary)],text=True);abi[name]=dict(sha256=sha(binary),elf64_little_endian_x86_64=True,readelf=details)
  save('linux-abi.json',abi);save('manifest.json',manifest)
 run([sys.executable,'-B',str(a.isolated_qa/'native.py'),'--archive',str(native),'--archive-sha256',native_sha,'--commit',a.commit,'--manager',str(manager_path),'--out',str(a.out/'native-runtime.json')],'native-runtime',timeout=900)
 canonical='ghcr.io/qiwolf/tokengo:sha-'+a.commit;local='tokengo:sha-'+a.commit;registry=(a.assets/'registry-image.txt').read_text().strip();assert re.fullmatch('ghcr\\.io/qiwolf/tokengo@sha256:[0-9a-f]{64}',registry)
 image_archive=a.assets/verified['image_archive']
 with tarfile.open(image_archive,'r:gz') as archive:
  items=json.load(archive.extractfile('manifest.json'));assert len(items)==1 and set(items[0]['RepoTags'])=={canonical,local};config_name=items[0]['Config'];assert not Path(config_name).is_absolute() and '..' not in Path(config_name).parts;member=archive.getmember(config_name);assert member.size<10000000;raw=archive.extractfile(member).read();config=json.loads(raw);image_id='sha256:'+hashlib.sha256(raw).hexdigest();assert config['config']['Labels']['org.opencontainers.image.revision']==a.commit;assert json.loads((a.assets/'image-manifest.json').read_text())['image_id']==image_id
 save('offline-image-identity.json',dict(image_id=image_id,source_commit=a.commit,aliases=[canonical,local]))
 with tempfile.TemporaryDirectory(prefix='q32-empty-docker-config-') as docker_config:
  env={**os.environ,'DOCKER_CONFIG':docker_config,'DOCKER_HOST':'unix:///var/run/docker.sock','PYTHONDONTWRITEBYTECODE':'1'}
  for name in ['DOCKER_CONTEXT','DOCKER_TLS_VERIFY','DOCKER_CERT_PATH','DOCKER_AUTH_CONFIG']:env.pop(name,None)
  for name in list(env):
   if name.startswith('TOKENGO_'):env.pop(name)
  env.update(TOKENGO_BIND='127.0.0.1',TOKENGO_PORT='0')
  def docker(*args):return subprocess.check_output(['docker',*args],env=env,text=True,stderr=subprocess.STDOUT).strip()
  def inspect(name):
   r=subprocess.run(['docker','image','inspect',name],env=env,capture_output=True,text=True);return json.loads(r.stdout)[0] if r.returncode==0 else None
  before={name:inspect(name) for name in [canonical,local,registry]}
  assert all(value is None or value['Id']==image_id for value in before.values()),'Existing unrelated alias would be overwritten'
  try:
   run(['docker','pull','--platform','linux/amd64',registry],'anonymous-registry-pull',env)
   pulled=inspect(registry);assert pulled['Id']==image_id and registry in pulled.get('RepoDigests',[]) and pulled['Config']['Labels']['org.opencontainers.image.revision']==a.commit and pulled['Os']=='linux' and pulled['Architecture']=='amd64'
   run(['docker','load','-i',str(image_archive)],'offline-image-load',env)
   assert all(inspect(name)['Id']==image_id for name in [canonical,local])
   compose=json.loads(docker('compose','--env-file','/dev/null','-p','q32-accept-'+a.commit[:12],'-f',str(a.assets/'compose.yaml'),'config','--format','json'));services=compose['services'];assert len(services)==1;service=next(iter(services.values()));assert service['image']==canonical and 'build' not in service;save('compose-config.json',dict(result='passed',empty_env_file=True,image=canonical,deployment_started=False))
   save('anonymous-registry.json',dict(image=registry,image_id=image_id,empty_docker_config=True,source_commit=a.commit))
   run([sys.executable,'-B',str(a.isolated_qa/'container.py'),'--archive',str(native),'--archive-sha256',native_sha,'--commit',a.commit,'--manager',str(manager_path),'--image',canonical,'--out',str(a.out/'container-runtime.json')],'container-runtime',env,timeout=1200)
  finally:
   cleanup=[]
   for name,old in before.items():
    if old is None and inspect(name) is not None:
     r=subprocess.run(['docker','image','rm',name],env=env,capture_output=True,text=True);cleanup.append(dict(reference=name,exit_code=r.returncode,absent=inspect(name) is None))
   save('image-reference-cleanup.json',cleanup)
 record.update(result='passed',assets=9,schema_version=47,native_key_restart=True,container_key_restart_recreate=True,anonymous_ghcr_pull=True,browser_behavior_executed=False)
except Exception as error:
 record.update(result='failed',failure_type=type(error).__name__,failure_message=str(error));raise
finally:
 record['elapsed_seconds']=round(time.monotonic()-started,3);record['qa_script_hashes']={str(p.relative_to(Path(__file__).parent)):sha(p) for p in Path(__file__).parent.rglob('*.py')};record['isolated_qa_hashes']={p.name:sha(p) for p in a.isolated_qa.glob('*.py')};save('RESULT.json',record)
print(json.dumps(record))
