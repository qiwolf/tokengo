"""Bind the existing repository smoke flow to one verified q32 archive and local image."""
from pathlib import Path
import argparse,hashlib,importlib.util,json,os,re,socket,subprocess,tempfile,sys

def main():
    ap=argparse.ArgumentParser();ap.add_argument('--archive',type=Path,required=True);ap.add_argument('--archive-sha256',required=True);ap.add_argument('--commit',required=True);ap.add_argument('--manager',type=Path,required=True);ap.add_argument('--image',required=True);ap.add_argument('--out',type=Path,required=True);args=ap.parse_args()
    assert socket.gethostname()=='build-test' and re.fullmatch('[0-9a-f]{40}',args.commit)
    assert hashlib.sha256(args.archive.read_bytes()).hexdigest()==args.archive_sha256
    spec=importlib.util.spec_from_file_location('release_manager',args.manager);manager=importlib.util.module_from_spec(spec);spec.loader.exec_module(manager)
    with tempfile.TemporaryDirectory(prefix='container-release-q32-') as temp:
        root,commit=manager.verify_bundle(args.archive,Path(temp));assert commit==args.commit
        env={**os.environ,'Q32_RELEASE_TREE':str(root),'Q32_EXPECTED_COMMIT':args.commit,'PYTHONDONTWRITEBYTECODE':'1','DOCKER_HOST':'unix:///var/run/docker.sock','HTTP_PROXY':'http://127.0.0.1:1','HTTPS_PROXY':'http://127.0.0.1:1','ALL_PROXY':'http://127.0.0.1:1','NO_PROXY':'127.0.0.1,localhost'}
        result=subprocess.run([sys.executable,'-B',str(Path(__file__).with_name('container_smoke.py')),args.image],env=env,capture_output=True,timeout=900)
        args.out.parent.mkdir(parents=True,exist_ok=True);args.out.with_suffix('.log').write_bytes(result.stdout+result.stderr)
        if result.returncode:raise RuntimeError('isolated container QA failed; inspect its local log')
        data=json.loads(result.stdout);data['archive_sha256']=args.archive_sha256;args.out.write_text(json.dumps(data,indent=2)+'\n');print(json.dumps(data))
if __name__=='__main__':main()
