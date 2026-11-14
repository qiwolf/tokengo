"""Fresh temporary Linux native/CLI/WASM/Key-restart acceptance. Never production or live upstreams."""
from pathlib import Path
import argparse,hashlib,importlib.util,json,os,platform,re,secrets,socket,sqlite3,subprocess,tempfile,time,urllib.request,urllib.error
from schema47 import check as schema_check

def request(base,path,body=None,key=None):
    assert re.fullmatch(r'http://127\.0\.0\.1:\d+',base)
    headers={'Content-Type':'application/json'}
    if key:headers['Authorization']='Bearer '+key
    req=urllib.request.Request(base+path,data=None if body is None else json.dumps(body).encode(),headers=headers)
    try:
        with urllib.request.build_opener(urllib.request.ProxyHandler({})).open(req,timeout=10) as response:return response.status,response.read()
    except urllib.error.HTTPError as error:return error.code,error.read()
def ready(base):
    for _ in range(90):
        try:
            code,raw=request(base,'/readyz');value=json.loads(raw)
            if code==200 and value.get('status')=='ok' and value.get('checks',{}).get('database')=='ok':return
        except (OSError,ValueError):pass
        time.sleep(1)
    raise RuntimeError('isolated native fixture did not become ready')
def sha(path):return hashlib.sha256(Path(path).read_bytes()).hexdigest()
def main():
    ap=argparse.ArgumentParser();ap.add_argument('--archive',type=Path,required=True);ap.add_argument('--archive-sha256',required=True);ap.add_argument('--commit',required=True);ap.add_argument('--manager',type=Path,required=True);ap.add_argument('--out',type=Path,required=True);args=ap.parse_args()
    assert socket.gethostname()=='build-test' and platform.system()=='Linux' and platform.machine()=='x86_64'
    assert re.fullmatch('[0-9a-f]{40}',args.commit) and sha(args.archive)==args.archive_sha256
    spec=importlib.util.spec_from_file_location('release_manager',args.manager);manager=importlib.util.module_from_spec(spec);spec.loader.exec_module(manager)
    args.out.parent.mkdir(parents=True,exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='native-runtime-q32-') as temp:
        release,commit=manager.verify_bundle(args.archive,Path(temp));assert commit==args.commit
        cli=subprocess.check_output([str(release/'bin/tokengo'),'version'],text=True,timeout=10).strip();assert cli.startswith('tokengo ')
        state=Path(temp)/'state';state.mkdir(mode=0o700)
        with socket.socket() as sock:sock.bind(('127.0.0.1',0));port=sock.getsockname()[1]
        base=f'http://127.0.0.1:{port}'
        env={name:os.environ[name] for name in ['PATH','LANG','LC_ALL','TZ'] if name in os.environ}
        env.update(TOKENGO_LISTEN=f'127.0.0.1:{port}',TOKENGO_DB=str(state/'tokengo.db'),TOKENGO_MASTER_KEY_FILE=str(state/'master.key'),TOKENGO_WEB_DIR=str(release/'web'),TOKENGO_LOG_DIR=str(state/'logs'),HTTP_PROXY='http://127.0.0.1:1',HTTPS_PROXY='http://127.0.0.1:1',ALL_PROXY='http://127.0.0.1:1',NO_PROXY='127.0.0.1,localhost')
        migrated=subprocess.run([str(release/'bin/tokengo-server'),'--migrate-only'],env=env,cwd=state,capture_output=True,timeout=120)
        assert migrated.returncode==0;receipt=json.loads(migrated.stdout);assert receipt['schema_version']==47 and receipt['status']=='ok' and receipt['mode']=='migrate-only'
        with socket.socket() as sock:assert sock.connect_ex(('127.0.0.1',port))!=0
        migration_schema=schema_check(state/'tokengo.db',release);process=None;key=None
        def start():
            nonlocal process
            process=subprocess.Popen([str(release/'bin/tokengo-server')],cwd=state,env=env,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL);ready(base)
        def stop():
            process.terminate();assert process.wait(timeout=150)==0
        def authenticate():
            assert request(base,'/v1/models')[0]==401
            code,raw=request(base,'/v1/models',key=key);assert code==200 and json.loads(raw)['data']==[]
        try:
            start();assert json.loads(request(base,'/api/auth/status')[1])['needs_setup']
            wasm=[]
            for path in (release/'web').rglob('*'):
                if path.is_file():
                    code,raw=request(base,'/ui/'+path.relative_to(release/'web').as_posix());assert code==200 and raw==path.read_bytes()
                    if path.suffix=='.wasm':assert raw[:4]==b'\0asm';wasm.append({'member':path.relative_to(release).as_posix(),'sha256':sha(path)})
            assert wasm;assert request(base,'/api/channels')[0]==401
            code,raw=request(base,'/api/auth/setup',{'password':secrets.token_urlsafe(32)});assert code==200;key=json.loads(raw)['gateway_key'];raw=None;authenticate()
            master=sha(state/'master.key');inode=(state/'tokengo.db').stat().st_ino;assert (state/'master.key').stat().st_mode&0o777==0o600
            with sqlite3.connect('file:'+str(state/'tokengo.db')+'?mode=ro',uri=True) as c:
                nonce,cipher=c.execute('SELECT nonce,cipher FROM gateway_keys WHERE is_default=1').fetchone();assert nonce and cipher and key.encode() not in cipher
            stop();start();authenticate();assert sha(state/'master.key')==master and (state/'tokengo.db').stat().st_ino==inode
            stop();key=None;runtime_schema=schema_check(state/'tokengo.db',release)
            out={'result':'passed','host':socket.gethostname(),'source_commit':args.commit,'archive_sha256':args.archive_sha256,'server_sha256':sha(release/'bin/tokengo-server'),'cli_sha256':sha(release/'bin/tokengo'),'cli_version':cli,'wasm':wasm,'migration_only':migration_schema,'post_runtime':runtime_schema,'key_encrypted_and_auth_survives_restart':True,'master_key0600_and_database_inode_preserved':True,'upstream_calls':0,'production_access':False,'browser_behavior_executed':False}
            args.out.write_text(json.dumps(out,indent=2)+'\n');print(json.dumps(out))
        finally:
            key=None
            if process is not None and process.poll() is None:process.terminate();process.wait(timeout=150)
if __name__=='__main__':main()
