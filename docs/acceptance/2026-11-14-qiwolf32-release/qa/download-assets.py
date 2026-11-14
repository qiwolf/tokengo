"""Download all nine public assets anonymously; validate official size and digest."""
import argparse,concurrent.futures,hashlib,json,pathlib,re,subprocess,time
p=argparse.ArgumentParser();p.add_argument('--release',type=pathlib.Path,required=True);p.add_argument('--out',type=pathlib.Path,required=True);a=p.parse_args();release=json.loads(a.release.read_text());assert release['tag_name']=='v0.34.0-qiwolf.32' and not release['draft'] and not release['prerelease'];assets=release['assets'];assert len(assets)==9;a.out.mkdir(parents=True,exist_ok=True)
def sha(path):
 h=hashlib.sha256()
 with path.open('rb') as f:
  for block in iter(lambda:f.read(1024*1024),b''):h.update(block)
 return 'sha256:'+h.hexdigest()
def download(row):
 name=row['name'];assert pathlib.Path(name).name==name and re.fullmatch('sha256:[0-9a-f]{64}',row['digest']);url=row['browser_download_url'];assert url.startswith('https://github.com/qiwolf/tokengo/releases/download/v0.34.0-qiwolf.32/')
 target=a.out/name;started=time.time();cached=target.exists() and target.stat().st_size==row['size'] and sha(target)==row['digest']
 if not cached:
  partial=a.out/(name+'.part');subprocess.run(['curl','-q','-4','--fail','--location','--silent','--show-error','--connect-timeout','20','--max-time','1200','--retry','2','--output',str(partial),url],check=True);assert partial.stat().st_size==row['size'] and sha(partial)==row['digest'];partial.replace(target)
 return dict(name=name,bytes=target.stat().st_size,digest=sha(target),github_size_digest_match=True,anonymous=True,reused_matching_download=cached,elapsed_seconds=round(time.time()-started,3))
results=[]
try:
 with concurrent.futures.ThreadPoolExecutor(max_workers=3) as pool:
  for result in pool.map(download,assets):results.append(result)
finally:(a.out.parent/'download-receipt.json').write_text(json.dumps(dict(release=release['html_url'],assets=results,expected=9,completed=len(results),anonymous=True),indent=2)+'\n')
assert len(results)==9;print(json.dumps(dict(downloaded=9,anonymous=True)))
