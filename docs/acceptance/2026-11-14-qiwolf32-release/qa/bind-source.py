"""Read-only Mac Git export of exact tag-source hashes; no checkout/commit/build."""
import argparse,hashlib,json,pathlib,re,subprocess
p=argparse.ArgumentParser();p.add_argument('--repo',required=True);p.add_argument('--commit',required=True);p.add_argument('--out',type=pathlib.Path,required=True);a=p.parse_args();assert re.fullmatch('[0-9a-f]{40}',a.commit)
def git(*args):return subprocess.check_output(['git','-C',a.repo,*args])
assert git('rev-parse',a.commit+'^{commit}').decode().strip()==a.commit
paths=git('ls-tree','-rz','--name-only',a.commit).decode().split('\0');selected=[x for x in paths if x.startswith('docs/') or x in ['deploy/manage.py','Cargo.lock','LICENSE','NOTICE','scripts/mineru-gateway-client.py','crates/server/src/db/request_paths.sql','crates/server/src/db/account_overview.sql','crates/server/src/db/query_performance.sql','crates/server/src/db/model_invocation_totals.sql','crates/server/src/db/usage_time.sql','crates/server/src/db/currency_units.sql']]
hashes={name:hashlib.sha256(git('show',a.commit+':'+name)).hexdigest() for name in selected};a.out.parent.mkdir(parents=True,exist_ok=True);a.out.write_text(json.dumps(dict(commit=a.commit,files=hashes),indent=2)+'\n');print(json.dumps(dict(commit=a.commit,files=len(hashes))))
