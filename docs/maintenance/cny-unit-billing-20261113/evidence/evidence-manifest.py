import hashlib,json,pathlib,sys
r=pathlib.Path(__file__).resolve().parent
out={str(p.relative_to(r)):hashlib.sha256(p.read_bytes()).hexdigest() for p in sorted(r.rglob('*')) if p.is_file() and p.name!='hashes.json'}
p=r/'hashes.json'
if '--write' in sys.argv:p.write_text(json.dumps(out,indent=2)+'\n')
else:
 assert json.loads(p.read_text())==out,'Evidence changed'
 print(str(len(out))+' evidence files verified')
