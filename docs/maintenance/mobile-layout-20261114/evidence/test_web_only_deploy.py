"""Isolated filesystem/lifecycle tests. Run on build-test; no production services."""
from pathlib import Path
import copy,hashlib,importlib.util,io,json,tarfile,tempfile,unittest
HERE=Path(__file__).resolve().parent
spec=importlib.util.spec_from_file_location('web_only_deploy',HERE/'deploy.py');module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module)
def sha(data):return hashlib.sha256(data).hexdigest()
def archive(files):
    out=io.BytesIO()
    with tarfile.open(fileobj=out,mode='w:gz') as tar:
        for name,data in files.items():
            member=tarfile.TarInfo(name);member.size=len(data);tar.addfile(member,io.BytesIO(data))
    return out.getvalue()
class FakeCommon:
    def __init__(self,root):
        self.BASE=root/'opt/tokengo';self.OLD=self.BASE/'releases/old';self.BACKUPS=root/'backups';self.calls=[];self.staged=False;self.fail_new_health=False;self.drift_after_stage=False
        (self.OLD/'bin').mkdir(parents=True);(self.OLD/'web').mkdir();(self.OLD/'deploy').mkdir()
        (self.OLD/'bin/tokengo-server').write_bytes(b'original server');(self.OLD/'bin/tokengo').write_bytes(b'original CLI');(self.OLD/'deploy/manage.py').write_bytes(b'original manager')
        (self.OLD/'source-manifest.json').write_bytes(b'original server source identity')
        (self.OLD/'web/index.html').write_bytes(b'<script src="old-11111111.js"></script>')
        (self.OLD/'web/old-11111111.js').write_bytes(b'old JS retained for pages already open')
        (self.OLD/'web/old-22222222.wasm').write_bytes(b'old wasm retained for pages already open')
        (self.OLD/'manifest.json').write_text(json.dumps({'commit':'0'*40,'schema_version':47,'target':'linux','local_patch':{'name':'retained-runtime-patch'}}))
        (self.OLD/'checksums.json').write_text(json.dumps(module.inventory(self.OLD)))
        (self.BASE/'current').symlink_to(self.OLD)
        self.business=root/'business.json';self.business.write_text(json.dumps({'ledger':[123],'routes':['unchanged']}))
        self.OLD_SHA=self.sha(self.OLD/'bin/tokengo-server')
        self.identity={'pid':'synthetic-pid-1','server_sha256':self.OLD_SHA,'environment_sha256':'original-environment','web_dir':str(self.BASE/'current/web'),'config_files':{'env':'original-config-hash'},'configuration':{'schema_version':47,'routes':'original-route-hash'}}
    def need(self,value,message):
        if not value:raise RuntimeError(message)
    def sha(self,path):return sha(Path(path).read_bytes())
    def lock(self):self.calls.append('lock');return object()
    def runtime_identity(self):
        value=copy.deepcopy(self.identity)
        if self.staged and self.drift_after_stage:value['environment_sha256']='externally-changed-environment'
        self.calls.append('runtime_identity');return value
    def write_result(self,gate,result):(gate/'result.json').write_text(json.dumps(result))
    def manager(self,release):
        common=self
        class Manager:
            def verify_files(self,release):
                actual=module.inventory(release);actual.pop('checksums.json',None)
                common.need(actual==json.loads((release/'checksums.json').read_text()),'checksum mismatch')
            def await_health(self):
                common.calls.append('http_exact_asset_health')
                current=(common.BASE/'current').resolve()
                if common.fail_new_health and current!=common.OLD:
                    value=json.loads(common.business.read_text());value['ledger'].append(17);common.business.write_text(json.dumps(value))
                    raise RuntimeError('new served asset did not match its bytes')
                self.verify_files(current)
            def sync_tree(self,release):common.staged=True;common.calls.append('sync_tree')
            def set_current(self,release):
                common.calls.append('atomic_current_switch');link=common.BASE/'current.next-test';link.symlink_to(release);link.replace(common.BASE/'current')
        manager=Manager();manager.verify_files(release);return manager
class WebOnlyDeploy(unittest.TestCase):
    def payload(self,d):
        files={'index.html':b'<script src="new-aaaaaaaa.js"></script>','new-aaaaaaaa.js':b'new JS','new-bbbbbbbb.wasm':b'new wasm','styles/new-cccccccc.css':b'.price{overflow-wrap:anywhere}'}
        web=archive(files);source='{"console-design.css":"new source hash"}\n'
        meta={'old_release':str(d.OLD),'server_sha256':d.OLD_SHA,'commit':'1'*40,'web_bytes':len(web),'web_sha256':sha(web),'web_files':{name:sha(data) for name,data in files.items()},'source_manifest':source,'source_manifest_sha256':sha(source.encode())}
        return meta,web
    def record(self,d):
        paths=list(d.BACKUPS.glob('web-only-*/result.json'));self.assertEqual(len(paths),1);return json.loads(paths[0].read_text())
    def retained(self,d):
        current=(d.BASE/'current').resolve()
        self.assertEqual((current/'bin/tokengo-server').read_bytes(),b'original server');self.assertEqual((current/'bin/tokengo').read_bytes(),b'original CLI')
        self.assertFalse(any(word in ' '.join(d.calls) for word in ['stop','start','drain','ingress','snapshot_restore']))
    def test_success_keeps_process_runtime_and_previous_hashed_assets(self):
        with tempfile.TemporaryDirectory() as tmp:
            d=FakeCommon(Path(tmp));meta,web=self.payload(d);before=d.runtime_identity();result=module.deploy(meta,web,d);current=(d.BASE/'current').resolve()
            self.assertTrue(result['executed']);self.assertEqual(result['before'],before);self.assertEqual(result['after'],before);self.assertNotEqual(current,d.OLD);self.retained(d)
            self.assertTrue((current/'web/old-11111111.js').is_file());self.assertTrue((current/'web/old-22222222.wasm').is_file());self.assertEqual((current/'web/styles').stat().st_mode&0o777,0o755)
            for name,digest in meta['web_files'].items():self.assertEqual(d.sha(current/'web'/name),digest)
            manifest=json.loads((current/'manifest.json').read_text());self.assertEqual(manifest['deployment_kind'],'web-only');self.assertEqual(manifest['commit'],'0'*40);self.assertEqual(manifest['web_source_commit'],'1'*40);self.assertEqual(manifest['local_patch']['name'],'retained-runtime-patch')
            self.assertEqual(json.loads(d.business.read_text())['ledger'],[123])
    def test_invalid_bundle_and_path_traversal_fail_before_switch(self):
        for unsafe in [False,True]:
            with self.subTest(unsafe=unsafe),tempfile.TemporaryDirectory() as tmp:
                d=FakeCommon(Path(tmp));meta,web=self.payload(d)
                if unsafe:
                    web=archive({'index.html':b'x','app-aaaaaaaa.wasm':b'x','../escape':b'forbidden'});meta.update(web_bytes=len(web),web_sha256=sha(web))
                else:meta['web_sha256']='0'*64
                with self.assertRaises((RuntimeError,ValueError)):module.deploy(meta,web,d)
                self.assertEqual((d.BASE/'current').resolve(),d.OLD);self.assertNotIn('atomic_current_switch',d.calls);self.retained(d)
                result=self.record(d);self.assertFalse(result['executed']);self.assertTrue(result['runtime_and_configuration_preserved']);self.assertEqual(result['before'],result['after']);self.assertFalse((d.BASE/'escape').exists())
    def test_failed_served_assets_roll_back_only_web_and_keep_new_business_writes(self):
        with tempfile.TemporaryDirectory() as tmp:
            d=FakeCommon(Path(tmp));d.fail_new_health=True;meta,web=self.payload(d)
            with self.assertRaisesRegex(RuntimeError,'served asset'):module.deploy(meta,web,d)
            result=self.record(d);self.assertEqual((d.BASE/'current').resolve(),d.OLD);self.assertTrue(result['automatically_rolled_back_web_only']);self.assertTrue(result['runtime_and_configuration_preserved']);self.assertEqual(result['before'],result['after']);self.retained(d)
            self.assertEqual(d.calls.count('atomic_current_switch'),2);self.assertEqual(json.loads(d.business.read_text())['ledger'],[123,17]);self.assertFalse(result['business_database_restored'])
    def test_external_runtime_drift_refuses_activation_without_restoring_configuration(self):
        with tempfile.TemporaryDirectory() as tmp:
            d=FakeCommon(Path(tmp));d.drift_after_stage=True;meta,web=self.payload(d)
            with self.assertRaisesRegex(RuntimeError,'PID/binary/environment'):module.deploy(meta,web,d)
            result=self.record(d);self.assertEqual((d.BASE/'current').resolve(),d.OLD);self.assertNotIn('atomic_current_switch',d.calls);self.assertFalse(result['runtime_and_configuration_preserved']);self.assertEqual(result['after']['pid'],result['before']['pid']);self.assertEqual(result['after']['environment_sha256'],'externally-changed-environment');self.retained(d)
            self.assertEqual(json.loads(d.business.read_text())['ledger'],[123])
if __name__=='__main__':unittest.main(verbosity=2)
