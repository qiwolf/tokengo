from pathlib import Path
from contextlib import contextmanager
import datetime,fcntl,hashlib,importlib.util,json,os,pwd,re,signal,sqlite3,subprocess,time,urllib.request,urllib.error
BASE=Path('/opt/tokengo')
DB=Path('/var/lib/tokengo/tokengo.db')
LOCK='/run/lock/tokengo-query-performance-deploy.lock'
OLD=BASE/'releases/no-local-budget-cbab724cf3cb-c23913c9'
OLD_SHA='c23913c900196e7208d76674c039549d222a26ae46c3127d638ff7184a1ed2d1'
BACKUPS=Path('/var/backups/tokengo')
def need(value,message):
    if not value: raise RuntimeError(message)

def run(*args):
    return subprocess.check_output(args,text=True,stderr=subprocess.PIPE).strip()

def sha(path):
    h=hashlib.sha256()
    with Path(path).open('rb') as f:
        for chunk in iter(lambda:f.read(1024*1024),b''):h.update(chunk)
    return h.hexdigest()

def lock():
    need(run('hostname')=='tokengo-geren-wangguan','wrong container')
    handle=open(LOCK,'a');fcntl.flock(handle,fcntl.LOCK_EX|fcntl.LOCK_NB)
    def interrupted(sig,frame):raise RuntimeError('deployment interrupted by signal '+str(sig))
    for sig in [signal.SIGINT,signal.SIGTERM,signal.SIGHUP]:signal.signal(sig,interrupted)
    return handle

def manager(release):
    spec=importlib.util.spec_from_file_location('tokengo_manage',release/'deploy/manage.py')
    m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m)
    m.verify_files(release);m.validate_config();return m

def config_hashes():
    roots=[Path('/etc/tokengo'),Path('/etc/systemd/system/tokengo.service')]
    return {str(p):sha(p) for root in roots for p in ([root] if root.is_file() else root.rglob('*')) if p.is_file()}

@contextmanager
def ro():
    c=sqlite3.connect('file:'+str(DB)+'?mode=ro',uri=True,timeout=2)
    try:
        c.execute('PRAGMA query_only=ON')
        yield c
    finally:
        # sqlite3.Connection.__exit__ commits/rolls back but does NOT close.
        # Release readers/WAL handles deterministically before offline migration.
        c.close()

def write_result(gate,result):
    gate=Path(gate);p=gate/'result.json';temporary=gate/('result.json.tmp-'+os.urandom(8).hex())
    with temporary.open('x') as f:
        f.write(json.dumps(result,sort_keys=True,indent=2)+'\n');f.flush();os.fsync(f.fileno())
    os.replace(temporary,p)
    fd=os.open(gate,os.O_RDONLY|os.O_DIRECTORY)
    try:os.fsync(fd)
    finally:os.close(fd)

def configuration_identity():
    # Hash static routing/source configuration in-process; never export values.
    with ro() as c:
        c.execute('BEGIN')
        schema=c.execute('SELECT MAX(version) FROM schema_migrations').fetchone()[0]
        hashes={}
        stable={'id','name','base_url','enabled','unwrap_envelope','max_concurrent','priority','binding_generation','transport_kind','model_name','upstream_model','vision','drop_fields','provider_id','channel_id','credential_id','config_json','version'}
        tables={row[0] for row in c.execute("SELECT name FROM sqlite_master WHERE type='table'")}
        for table in ['channels','channel_models','provider_channels','provider_credentials','provider_connections','credential_models','shared_virtual_models']:
            if table not in tables:continue
            cols=[row[1] for row in c.execute('PRAGMA table_info('+table+')') if row[1] in stable]
            if not cols:continue
            rows=c.execute('SELECT '+','.join(cols)+' FROM '+table+' ORDER BY '+','.join(cols)).fetchall()
            hashes[table]=hashlib.sha256(json.dumps([cols,rows],sort_keys=True).encode()).hexdigest()
        rows=c.execute("SELECT key,value FROM settings WHERE key LIKE 'model_routing:%' OR key LIKE 'channel_protocols:%' ORDER BY key").fetchall()
        hashes['routing_protocol_settings']=hashlib.sha256(json.dumps(rows,sort_keys=True).encode()).hexdigest()
        return {'schema_version':schema,'static_configuration_hashes':hashes}

def runtime_identity():
    pid=run('systemctl','show','tokengo','-p','MainPID','--value')
    need(int(pid)>0,'no running server')
    environment=Path('/proc/'+pid+'/environ').read_bytes()
    env=dict(item.split(b'=',1) for item in environment.split(b'\0') if b'=' in item)
    return {'pid':pid,'server_sha256':sha('/proc/'+pid+'/exe'),
        'environment_sha256':hashlib.sha256(b'\0'.join(sorted(x for x in environment.split(b'\0') if x))).hexdigest(),
        'web_dir':env.get(b'TOKENGO_WEB_DIR',b'').decode(),
        'config_files':config_hashes(),'configuration':configuration_identity()}
