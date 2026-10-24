import sys,pathlib,sqlite3,json,os
os.umask(0o077);folder=pathlib.Path(sys.argv[1]).resolve();execute='--execute' in sys.argv[2:]
assert folder.parent==pathlib.Path('/var/backups/tokengo') and folder.name.startswith('billing-test-cleanup-models-')
assert pathlib.Path('/etc/hostname').read_text().strip()=='tokengo-geren-wangguan'
plan=json.loads((folder/'scope.json').read_text());before=sqlite3.connect('file:'+str(folder/'before.db')+'?mode=ro',uri=True);live=sqlite3.connect('/var/lib/tokengo/tokengo.db',timeout=10)
rows=[]
for table,column,ids in [('settings','key',plan['setting_keys']),('model_kinds','model_name',plan['model_names'])]:
 for id in ids:
  prior=before.execute('SELECT * FROM '+table+' WHERE '+column+'=?',(id,)).fetchone();assert prior is not None
  assert live.execute('SELECT 1 FROM '+table+' WHERE '+column+'=?',(id,)).fetchone() is None,'configuration changed after cleanup'
  rows.append((table,prior))
if execute:
 live.execute('BEGIN IMMEDIATE')
 try:
  for table,row in rows:live.execute('INSERT INTO '+table+' VALUES('+','.join('?'*len(row))+')',row)
  assert not live.execute('PRAGMA foreign_key_check').fetchall();live.commit()
 except BaseException:live.rollback();raise
print(json.dumps({'execute':execute,'delta_rows':len(rows),'history_rewind':False,'restored':execute}));live.close();before.close()
