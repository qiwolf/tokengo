import sqlite3,json,socket,pathlib,datetime,hashlib,subprocess,sys,os
assert socket.gethostname()=='tokengo-geren-wangguan';os.umask(0o077)
expected=json.load(sys.stdin);pid=subprocess.check_output(['systemctl','show','tokengo','-p','MainPID','--value'],text=True).strip();assert hashlib.sha256(pathlib.Path('/proc/'+pid+'/exe').read_bytes()).hexdigest()==expected['server_sha256'],'wrong deployed binary'
c=sqlite3.connect('/var/lib/tokengo/tokengo.db',timeout=10);c.row_factory=sqlite3.Row;c.execute('pragma synchronous=FULL');c.execute('begin immediate')
before=[dict(r) for r in c.execute('select * from billing_budgets where frozen<>0 order by scope,scope_id,currency')]
backup=pathlib.Path('/var/backups/tokengo')/('budget-enforcement-removed-'+datetime.datetime.now(datetime.timezone.utc).strftime('%Y%m%dT%H%M%SZ'));backup.mkdir(mode=0o700)
with (backup/'before.json').open('w') as f:json.dump(before,f,indent=2);f.flush();os.fsync(f.fileno())
try:
 for row in before:
  n=c.execute('update billing_budgets set frozen=0 where scope=? and scope_id=? and currency=? and frozen=?',(row['scope'],row['scope_id'],row['currency'],row['frozen'])).rowcount;assert n==1
  c.execute('insert into billing_audit(event,actor,object,detail,created_ms) values(?,?,?,?,?)',('budget_enforcement_removed','codex:user-authorized-no-local-budget',f"{row['scope']}:{row['scope_id']}:{row['currency']}",json.dumps({'old_frozen':row['frozen'],'new_frozen':0,'backup':str(backup),'prices_and_spent_preserved':True}),int(datetime.datetime.now().timestamp()*1000)))
 for row in before:
  actual=dict(c.execute('select * from billing_budgets where scope=? and scope_id=? and currency=?',(row['scope'],row['scope_id'],row['currency'])).fetchone());assert actual==dict(row,frozen=0)
 assert c.execute('select count(*) from billing_budgets where frozen<>0').fetchone()[0]==0
 c.commit()
except:c.rollback();raise
out={'executed':True,'host':socket.gethostname(),'backup':str(backup),'cleared':[{'scope':r['scope'],'scope_id':r['scope_id'],'currency':r['currency']} for r in before],'historical_prices_ledger_and_spent_unchanged':True,'rollback':'only compare-and-swap these flag rows using protected before.json; never restore business DB or rewrite past charges'};(backup/'result.json').write_text(json.dumps(out,indent=2));print(json.dumps(out))
