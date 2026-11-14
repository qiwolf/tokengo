"""Web-only release overlay: no API stop/start, ingress closure, DB writes or DB restore."""
from pathlib import Path, PurePosixPath
import datetime,hashlib,importlib.util,io,json,os,re,shutil,sys,tarfile

def digest(data):return hashlib.sha256(data).hexdigest()
def inventory(root):return {p.relative_to(root).as_posix():digest(p.read_bytes()) for p in root.rglob('*') if p.is_file()}
def bundle_files(meta,web):
    if len(web)!=meta['web_bytes'] or digest(web)!=meta['web_sha256']:raise ValueError('web bundle length/hash mismatch')
    expected=meta['web_files']
    if not isinstance(expected,dict) or 'index.html' not in expected or not any(name.endswith('.wasm') for name in expected):raise ValueError('bundle requires index and wasm inventory')
    files={};seen=set();total=0
    with tarfile.open(fileobj=io.BytesIO(web),mode='r:gz') as tar:
        for member in tar.getmembers():
            name=member.name.removeprefix('./');path=PurePosixPath(name)
            if name in ('','.'):continue
            if path.is_absolute() or '..' in path.parts or '\\' in name or name in seen or not(member.isfile() or member.isdir()):raise ValueError('unsafe/duplicate web archive entry')
            seen.add(name)
            if member.isdir():continue
            total+=member.size
            if member.size<0 or total>512*1024**2:raise ValueError('web expansion limit exceeded')
            files[name]=tar.extractfile(member).read()
    if {name:digest(data) for name,data in files.items()}!=expected:raise ValueError('web inventory mismatch')
    return files

def deploy(meta,web,d):
    os.umask(0o077);held=d.lock()
    gate=d.BACKUPS/('web-only-'+datetime.datetime.now(datetime.timezone.utc).strftime('%Y%m%dT%H%M%S%fZ'));gate.mkdir(mode=0o700,parents=True)
    record={'deployment_kind':'web-only','executed':False,'gate':str(gate),'business_database_restored':False,'api_restarted':False,'ingress_closed':False}
    before=d.runtime_identity();record['before']=before;old=d.OLD;new=None;switched=False
    def invariant():
        after=d.runtime_identity();record['after']=after;record['runtime_and_configuration_preserved']=after==before
        d.need(after==before,'API PID/binary/environment/configuration changed')
    try:
        d.need(meta['old_release']==str(old) and meta['server_sha256']==d.OLD_SHA,'metadata baseline mismatch')
        d.need((d.BASE/'current').is_symlink() and (d.BASE/'current').resolve()==old,'current release baseline changed')
        d.need(before['server_sha256']==d.OLD_SHA,'running server baseline changed')
        d.need(before['web_dir']==str(d.BASE/'current/web'),'web serving must use logical current/web path')
        d.need(re.fullmatch('[0-9a-f]{40}',meta['commit']) is not None,'invalid source commit')
        d.need(digest(meta['source_manifest'].encode())==meta['source_manifest_sha256'],'source manifest hash mismatch')
        files=bundle_files(meta,web);manager=d.manager(old);manager.await_health()
        old_web=inventory(old/'web');old_runtime={name:sha for name,sha in inventory(old).items() if not name.startswith('web/') and name not in ('manifest.json','checksums.json')}
        for name,data in files.items():
            if name in old_web and re.search(r'[0-9a-fA-F]{8,}',PurePosixPath(name).name):
                d.need(digest(data)==old_web[name],'changed content at an existing hashed asset path')
        d.need(shutil.disk_usage(old.parent).free>sum(p.stat().st_size for p in old.rglob('*') if p.is_file())+len(web)+256*1024**2,'insufficient staging space')
        new=d.BASE/'releases'/('web-mobile-'+meta['commit'][:12]+'-'+meta['web_sha256'][:8]);d.need(not new.exists(),'candidate already exists')
        shutil.copytree(old,new)
        for name,data in files.items():
            target=new/'web'/name;target.parent.mkdir(parents=True,exist_ok=True,mode=0o755);target.write_bytes(data);target.chmod(0o644)
        manifest=json.loads((old/'manifest.json').read_text())
        # Keep the inherited runtime commit/server/CLI/migration identity truthful.
        manifest.update(deployment_kind='web-only',web_source_commit=meta['commit'],web_bundle_sha256=meta['web_sha256'],
            web_source_manifest_sha256=meta['source_manifest_sha256'],retained_runtime_release=str(old),server_sha256=d.OLD_SHA,
            previous_runtime_manifest={'release':old.name,'sha256':d.sha(old/'manifest.json')},retained_hashed_assets=True)
        (new/'manifest.json').write_text(json.dumps(manifest,sort_keys=True,indent=2)+'\n')
        (new/'web-source-manifest.json').write_text(meta['source_manifest']);(new/'web-source-manifest.json').chmod(0o644)
        for directory in (path for path in new.rglob('*') if path.is_dir()):directory.chmod(0o755)
        new.chmod(0o755)
        checks=inventory(new);checks.pop('checksums.json',None);(new/'checksums.json').write_text(json.dumps(checks,sort_keys=True,indent=2)+'\n')
        for name,sha in old_runtime.items():d.need(d.sha(new/name)==sha,'inherited runtime/CLI/deploy asset changed')
        for name,sha in meta['web_files'].items():d.need(d.sha(new/'web'/name)==sha,'new web asset changed')
        for name,sha in old_web.items():
            if name not in files:d.need(d.sha(new/'web'/name)==sha,'previous web asset was removed')
        manager.verify_files(new);manager.sync_tree(new);invariant()
        d.need((d.BASE/'current').resolve()==old,'current changed before atomic switch')
        record.update(previous=str(old),target=str(new),web_sha256=meta['web_sha256'],web_files=meta['web_files'],phase='candidate_verified')
        d.write_result(gate,record)
        manager.set_current(new);switched=True
        # Existing server reads cfg.web_dir per request. Verify exact new assets
        # through the running HTTP service, including retained previous hashed files.
        manager.await_health();invariant()
        record.update(executed=True,phase='complete');d.write_result(gate,record);return record
    except BaseException as error:
        record.update(error_type=type(error).__name__,error=str(error),phase='failed')
        try:
            if new is not None and (d.BASE/'current').resolve()==new:
                d.manager(old).set_current(old);record['automatically_rolled_back_web_only']=True
                d.manager(old).await_health()
            invariant()
        except BaseException as recovery:
            record['recovery_error_type']=type(recovery).__name__;record['recovery_error']=str(recovery)
        d.write_result(gate,record);raise

def main():
    stream=sys.stdin.buffer;n=int.from_bytes(stream.read(4),'big')
    if not 0<n<2*1024**2:raise ValueError('invalid metadata length')
    meta=json.loads(stream.read(n));count=meta['web_bytes']
    if not isinstance(count,int) or not 0<count<256*1024**2:raise ValueError('invalid web length')
    web=stream.read(count)
    if stream.read(1):raise ValueError('unexpected trailing payload')
    # Root provides the reviewed common adapter; it has no stop/start/DB-write API.
    import tempfile
    with tempfile.TemporaryDirectory(prefix='tokengo-web-deploy-common-') as tmp:
        path=Path(tmp)/'common.py';path.write_text(meta['common'])
        spec=importlib.util.spec_from_file_location('tokengo_web_deploy_common',path);d=importlib.util.module_from_spec(spec);spec.loader.exec_module(d)
        result=deploy(meta,web,d)
    print(json.dumps(result,sort_keys=True))
if __name__=='__main__':main()
