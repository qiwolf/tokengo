"""Reversible app-only model mapping deployment from tested local commit; no schema/data migrations."""
from pathlib import Path
import sys,os,json,hashlib,subprocess,sqlite3,time,shutil,tarfile,io,importlib.util,urllib.request,urllib.error,re,datetime,signal
os.umask(0o077)
def need(x,msg):
 if not x:raise RuntimeError(msg)
def sha(p):return hashlib.sha256(Path(p).read_bytes()).hexdigest()
def run(*args):return subprocess.check_output(args,text=True).strip()
raw=sys.stdin.buffer;cfg=json.loads(raw.read(int.from_bytes(raw.read(4),'big')));archive=raw.read();need(hashlib.sha256(archive).hexdigest()==cfg['artifact_sha256'],'artifact checksum')
need(run('hostname')=='tokengo-geren-wangguan','wrong container')
base=Path('/opt/tokengo');old=(base/'current').resolve();need(str(old)==cfg['expected_current'],'release drift')
spec=importlib.util.spec_from_file_location('manage',old/'deploy/manage.py');m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m);m.verify_files(old)
oldpid=run('systemctl','show','tokengo','-p','MainPID','--value');oldexe=sha('/proc/'+oldpid+'/exe');need(oldexe==cfg['expected_server_sha256'],'running production binary drift');oldenv=hashlib.sha256(b'\0'.join(sorted(v for v in Path('/proc/'+oldpid+'/environ').read_bytes().split(b'\0') if v.startswith(b'TOKENGO_')))).hexdigest()
DB=Path('/var/lib/tokengo/tokengo.db')
def state():
 c=sqlite3.connect('file:'+str(DB)+'?mode=ro',uri=True)
 try:
  need(c.execute('pragma integrity_check').fetchone()[0]=='ok' and not c.execute('pragma foreign_key_check').fetchall(),'DB integrity')
  tables=['settings','panel_accounts','passthrough_acl','passthrough_headers','users','gateway_keys','billing_budgets','billing_prices','billing_request_prices','billing_ledger','billing_allocations','channels','channel_models','provider_channels','provider_connections','provider_credentials','source_credentials','passthrough_routes','shared_virtual_models','user_virtual_models','user_virtual_targets','user_virtual_windows']
  hashes={t:hashlib.sha256(repr(c.execute('SELECT * FROM "'+t+'" ORDER BY rowid').fetchall()).encode()).hexdigest() for t in tables}
  schema=hashlib.sha256(repr(c.execute('SELECT type,name,tbl_name,sql FROM sqlite_master ORDER BY type,name').fetchall()).encode()).hexdigest()
  return {'table_hashes':hashes,'schema':schema}
 finally:c.close()
def quiet():
 c=sqlite3.connect('file:'+str(DB)+'?mode=ro',uri=True)
 try:return c.execute("SELECT COUNT(*) FROM requests WHERE status='running'").fetchone()[0]==0 and c.execute('SELECT COUNT(*) FROM billing_ledger WHERE settled_ms IS NULL').fetchone()[0]==0 and c.execute('SELECT COALESCE(SUM(ABS(reserved_micro)),0) FROM billing_budgets').fetchone()[0]==0
 finally:c.close()
identity='model-mapping-'+cfg['commit'][:12];target=base/'releases'/identity;need(not target.exists(),'release exists')
shutil.copytree(old,target)
with tarfile.open(fileobj=io.BytesIO(archive),mode='r:gz') as t:
 for member in t.getmembers():need(member.isfile() and member.name in cfg['artifact_files'],'unexpected artifact member')
 # Retain old hashed assets for clients which fetched the old index before cutover.
 (target/'web').mkdir(exist_ok=True)
 for member in t.getmembers():
  dest=target/member.name;need('..' not in Path(member.name).parts and not Path(member.name).is_absolute(),'unsafe path');dest.parent.mkdir(parents=True,exist_ok=True);dest.write_bytes(t.extractfile(member).read());dest.chmod(0o755 if member.name.startswith('bin/') else 0o644)
for p in [target,*target.rglob('*')]:
 if p.is_dir():p.chmod(0o755)
manifest=json.loads((target/'manifest.json').read_text());manifest.update({'deployment_identity':identity,'public_release':False,'source_manifest_sha256':cfg['source_manifest_sha256'],'model_mapping_source_commit':cfg['commit'],'artifact_kind':'tested-local-git-commit','component_provenance':{'server':'build-test verified model mapping commit','web':'build-test verified model mapping commit','cli':'unchanged from previous release'}})
(target/'manifest.json').write_text(json.dumps(manifest,indent=2)+'\n');(target/'checksums.json').write_text(json.dumps({str(p.relative_to(target)):sha(p) for p in target.rglob('*') if p.is_file() and p.name!='checksums.json'},indent=2)+'\n');m.verify_files(target);m.sync_tree(target)
# Graceful ingress gate: old workers finish their existing requests.
site=Path('/etc/nginx/sites-enabled/tokengo').resolve();original=site.read_bytes();mode=site.stat().st_mode&0o777
need(len(re.findall(r'\bserver\s*\{',original.decode()))==1,'unexpected nginx site')
maintenance=re.sub(r'\bserver\s*\{','server {\n    # model-mapping-maintenance\n    return 503;',original.decode(),count=1).encode()
stamp=datetime.datetime.now(datetime.timezone.utc).strftime('%Y%m%dT%H%M%SZ');gate=Path('/var/backups/tokengo')/('model-mapping-deploy-ingress-'+stamp);gate.mkdir(mode=0o700);(gate/'nginx-site.before').write_bytes(original)
restorecode="import pathlib,subprocess; p=pathlib.Path("+repr(str(site))+"); p.write_bytes(pathlib.Path("+repr(str(gate/'nginx-site.before'))+").read_bytes()); p.chmod("+repr(mode)+"); subprocess.run(['nginx','-t'],check=True,capture_output=True); subprocess.run(['systemctl','reload','nginx'],check=True,capture_output=True)"
restore=gate/'restore.py';restore.write_text(restorecode)
timer='tokengo-model-mapping-deploy-'+stamp.lower();subprocess.run(['systemd-run','--quiet','--unit='+timer,'--on-active=240s','/usr/bin/python3','-B',str(restore)],check=True)
def http_status(path):
 try:
  with urllib.request.urlopen('http://127.0.0.1'+path,timeout=5) as response:return response.status
 except urllib.error.HTTPError as e:return e.code
 except OSError:return 0
def wait_status(code):
 for _ in range(60):
  if http_status('/readyz')==code:return
  time.sleep(.2)
 raise RuntimeError('ingress status did not reach '+str(code))
def restore_ingress():
 subprocess.run(['/usr/bin/python3','-B',str(restore)],check=True,capture_output=True);wait_status(200);need(site.read_bytes()==original,'nginx config not restored');subprocess.run(['systemctl','stop',timer+'.timer'],capture_output=True)
def interrupted(sig,frame):raise RuntimeError('deployment interrupted '+str(sig))
for sig in [signal.SIGTERM,signal.SIGHUP,signal.SIGINT]:signal.signal(sig,interrupted)
result={'executed':False,'previous':str(old),'target':str(target),'source_manifest_sha256':cfg['source_manifest_sha256'],'public_release':False,'source_commit':cfg['commit'],'previous_server_sha256':oldexe};backup=None;stopped=False;opened=False
try:
 site.write_bytes(maintenance);site.chmod(mode);subprocess.run(['nginx','-t'],check=True,capture_output=True);subprocess.run(['systemctl','reload','nginx'],check=True,capture_output=True);wait_status(503);time.sleep(.5)
 for n in range(180):
  if quiet():break
  time.sleep(1)
 else:raise RuntimeError('active requests did not drain; refusing interruption')
 subprocess.run(['systemctl','stop','tokengo'],check=True);stopped=True;before=state();backup=m.make_snapshot();m.verify_files(backup);result['backup']=str(backup)
 m.set_current(target);subprocess.run(['systemctl','start','tokengo'],check=True);m.await_health()
 # No schema migration or business state change is required by this feature.
 after=state();need(before==after,'schema/business data changed at startup')
 pid=run('systemctl','show','tokengo','-p','MainPID','--value');need(sha('/proc/'+pid+'/exe')==sha(target/'bin/tokengo-server'),'wrong running binary')
 env=hashlib.sha256(b'\0'.join(sorted(v for v in Path('/proc/'+pid+'/environ').read_bytes().split(b'\0') if v.startswith(b'TOKENGO_')))).hexdigest();need(env==oldenv,'runtime configuration drift')
 restore_ingress();opened=True
 for name,digest in cfg['artifact_files'].items():
  if name.startswith('web/'):
   with urllib.request.urlopen('http://127.0.0.1/ui/'+name[4:],timeout=20) as r:need(hashlib.sha256(r.read()).hexdigest()==digest,'served web mismatch')
 result.update({'executed':True,'business_and_schema_unchanged':True,'running_server_sha256':sha('/proc/'+pid+'/exe'),'runtime_config_unchanged':True,'served_assets_verified':True,'graceful_ingress_restored':True,'no_forced_request_termination':True,'rollback':'restore previous current symlink and restart tokengo; retain current live database'})
except BaseException as e:
 result['error']=str(e)
 if stopped and not opened:
  subprocess.run(['systemctl','stop','tokengo'],check=True);m.set_current(old);subprocess.run(['systemctl','start','tokengo'],check=True);m.await_health();need(sha('/proc/'+run('systemctl','show','tokengo','-p','MainPID','--value')+'/exe')==oldexe,'rollback binary mismatch');result['rolled_back']=True
 if opened:result["rollback_deferred_to_preserve_new_requests"]=True
 restore_ingress();print(json.dumps(result));raise
print(json.dumps(result))
