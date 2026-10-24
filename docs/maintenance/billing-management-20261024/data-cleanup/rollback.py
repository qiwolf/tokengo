"""Restore only exact configuration deltas of one cleanup, never rewind history."""
import sqlite3,pathlib,json,sys,datetime,os
os.umask(0o077)
folder=pathlib.Path(sys.argv[1]).resolve();execute='--execute' in sys.argv[2:]
assert folder.parent==pathlib.Path('/var/backups/tokengo') and folder.name.startswith('billing-test-cleanup-')
assert pathlib.Path('/etc/hostname').read_text().strip()=='tokengo-geren-wangguan'
report=json.loads((folder/'report.json').read_text());plan=json.loads((folder/'cleanup-plan.json').read_text())
U={r['id'] for r in plan['test_users']};K={r['id'] for r in plan['test_keys']};C=set(plan['delete_source_ids']);P=set(plan['delete_provider_ids'])
a=sqlite3.connect('file:'+str(folder/'before.db')+'?mode=ro',uri=True);b=sqlite3.connect('file:'+str(folder/'after.db')+'?mode=ro',uri=True)
c=sqlite3.connect('/var/lib/tokengo/tokengo.db',timeout=10)
def table(t):return '"'+t.replace('"','""')+'"'
def snap(db,t):
 info=db.execute('PRAGMA table_info('+table(t)+')').fetchall()
 if not info:return [],[],{}
 cols=[r[1] for r in info];pk=[r[1] for r in sorted(info,key=lambda r:r[5]) if r[5]]
 if not pk:cols=['rowid']+cols;pk=['rowid'];sql='SELECT rowid,* FROM '+table(t)
 else:sql='SELECT * FROM '+table(t)
 ix=[cols.index(k) for k in pk]
 return cols,pk,{tuple(r[i] for i in ix):tuple(r) for r in db.execute(sql)}
def relevant(t,cols,r):
 if r is None:return False
 d=dict(zip(cols,r))
 if t=='settings':
  k=d['key']
  if k.startswith('deleted_user:'):return int(k.split(':')[-1]) in U
  if k.startswith(('deleted_key:','self_paused_key:')):return int(k.split(':')[-1]) in K
  return 'model_routing:' in k and 'ct104' in k
 if t=='users':return d['id'] in U
 if t=='gateway_keys':return d['id'] in K
 if t=='billing_budgets':return (d['scope']=='user' and d['scope_id'] in U)or(d['scope']=='key' and d['scope_id'] in K)
 if t=='provider_channels':return d['id'] in P
 if t=='channels':return d['id'] in C
 if t=='provider_credentials':return d['provider_id'] in P
 if t=='source_credentials':return d['source_id'] in C
 if 'channel_id' in d:return d['channel_id'] in C
 if t=='credential_affinity':return d.get('provider_id') in P or d.get('user_id') in U
 if t=='routing_state':return d.get('active_channel_id') in C
 if t=='protocol_sessions':
  try:return json.loads(d['pin_json']).get('channel') in C
  except Exception:return False
 if 'user_id' in d:return d['user_id'] in U
 if 'provider_id' in d:return d['provider_id'] in P
 return True
changes=[]
for t in report['rollback_tables']:
 cols,pk,old=snap(a,t);cols2,pk2,new=snap(b,t)
 if not cols:continue
 assert cols==cols2 and pk==pk2,'schema changed'
 _,_,live=snap(c,t)
 for key in old.keys()|new.keys():
  if old.get(key)==new.get(key):continue
  if not(relevant(t,cols,old.get(key))or relevant(t,cols,new.get(key))):continue
  assert live.get(key)==new.get(key),'later configuration change: '+t
  changes.append((t,cols,pk,key,old.get(key),new.get(key)))
summary={'execute':execute,'configuration_delta_rows':len(changes),'history_rewind':False,'tables':{t:sum(x[0]==t for x in changes) for t in sorted({x[0] for x in changes})}}
if execute:
 assert c.execute("SELECT COUNT(*) FROM requests WHERE status='running'").fetchone()[0]==0
 assert c.execute('SELECT COUNT(*) FROM billing_ledger WHERE settled_ms IS NULL').fetchone()[0]==0
 c.execute('PRAGMA foreign_keys=ON');c.execute('BEGIN IMMEDIATE');c.execute('PRAGMA defer_foreign_keys=ON')
 try:
  # Restore existing rows with UPDATE; insert deleted parents before their children.
  for t,cols,pk,key,old,new in changes:
   if old is None:continue
   if new is None:
    c.execute('INSERT INTO '+table(t)+'('+','.join(table(x) for x in cols)+') VALUES('+','.join('?'*len(cols))+')',old)
   else:
    sql='UPDATE '+table(t)+' SET '+','.join(table(x)+'=?' for x in cols)+' WHERE '+' AND '.join(table(x)+'=?' for x in pk)
    c.execute(sql,old+key)
  for t,cols,pk,key,old,new in reversed(changes):
   if old is None:c.execute('DELETE FROM '+table(t)+' WHERE '+' AND '.join(table(x)+'=?' for x in pk),key)
  # Secret insert triggers can bump restored channel fault revisions; reset the
  # original parent metadata once all child rows have been restored.
  for t,cols,pk,key,old,new in changes:
   if t=='channels' and old is not None:
    c.execute('UPDATE '+table(t)+' SET '+','.join(table(x)+'=?' for x in cols)+' WHERE '+' AND '.join(table(x)+'=?' for x in pk),old+key)
  for t in report['rollback_tables']:
   cols,pk,want=snap(a,t)
   if not cols:continue
   _,_,live=snap(c,t)
   wanted={k:v for k,v in want.items() if relevant(t,cols,v)}
   actual={k:v for k,v in live.items() if relevant(t,cols,v)}
   assert wanted==actual,'restored configuration mismatch: '+t
  assert not c.execute('PRAGMA foreign_key_check').fetchall(),'foreign key check failed'
  c.commit();summary['restored']=True
 except BaseException:c.rollback();raise
 (folder/'rollback-report.json').write_text(json.dumps(summary,indent=2)+'\n')
print(json.dumps(summary));c.close();a.close();b.close()
