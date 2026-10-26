"""Exercise real snapshot queries against synthetic SQLite; no network or credentials."""
import importlib.util
import pathlib
import sqlite3
import tempfile
import time
import unittest

spec = importlib.util.spec_from_file_location('gpu_panel', pathlib.Path(__file__).with_name('app.py'))
app = importlib.util.module_from_spec(spec)
spec.loader.exec_module(app)

class KeyLabels(unittest.TestCase):
    def test_snapshot_labels_and_key_drilldown(self):
        with tempfile.TemporaryDirectory() as tmp:
            app.TOKENGO_SNAP = str(pathlib.Path(tmp) / 'snapshot.db')
            db = sqlite3.connect(app.TOKENGO_SNAP)
            db.executescript('''
                CREATE TABLE meta(key TEXT,value TEXT);
                CREATE TABLE requests(ts_ms INTEGER,status TEXT,input_tokens INTEGER,output_tokens INTEGER,
                    input_tokens_known INTEGER,output_tokens_known INTEGER,latency_ms INTEGER,
                    served_model TEXT,channel_id INTEGER,model TEXT,key_id INTEGER,user_id INTEGER);
                CREATE TABLE billing_ledger(created_ms INTEGER,settled_ms INTEGER,user_id INTEGER,key_id INTEGER,
                    debit_micro INTEGER,billing_unit TEXT,model TEXT);
                CREATE TABLE gateway_keys(id INTEGER,name TEXT);
                CREATE TABLE users(id INTEGER,name TEXT);
                CREATE TABLE panel_accounts(user_id INTEGER,username TEXT);
            ''')
            now = int(time.time()*1000)
            db.execute('INSERT INTO meta VALUES(?,?)', ('exported_at_ms',str(now)))
            db.executemany('INSERT INTO users VALUES(?,?)', [(1,'Cyber'),(2,'Andy'),(3,None)])
            db.executemany('INSERT INTO gateway_keys VALUES(?,?)', [(10,'Memory'),(20,'Memory'),(30,'旧 Key'),(40,'<应用>')])
            db.executemany('INSERT INTO panel_accounts VALUES(?,?)', [(1,'cyber-login'),(2,'andy-login')])
            for uid,kid in [(1,10),(2,20),(3,30),(1,40),(99,99)]:
                db.execute('INSERT INTO requests VALUES(?,?,?,?,?,?,?,?,?,?,?,?)', (now,'ok',2,3,1,1,10,'model',1,'model',kid,uid))
                db.execute('INSERT INTO billing_ledger VALUES(?,?,?,?,?,?,?)', (now,now,uid,kid,1000,'token','model'))
            db.commit();db.close()
            app.tokengo_cache={'data':None,'ts':0}
            result=app.tokengo_stats()
            self.assertTrue(result['ok'],result)
            keys={k['key']:k for k in result['keys']}
            self.assertEqual(keys['10']['key_alias'],'Memory - Cyber')
            self.assertEqual(keys['20']['key_alias'],'Memory - Andy')
            self.assertEqual(keys['10']['key_name'],'Memory')
            self.assertEqual(keys['10']['user_alias'],'Cyber')
            self.assertEqual(keys['10']['email'],'cyber-login')
            self.assertEqual(keys['30']['key_alias'],'旧 Key')
            self.assertEqual(keys['40']['key_alias'],'<应用> - Cyber')
            self.assertEqual(keys['99']['key_alias'],'')
            self.assertEqual(result['req'],5)
            self.assertAlmostEqual(result['spend'],.005)
            self.assertEqual(keys['10']['req'],1)
            self.assertAlmostEqual(keys['10']['spend'],.001)
            drilldown=app.tokengo_key_models(10)
            self.assertEqual(drilldown['models'][0]['req'],1)
            self.assertEqual(drilldown['models'][0]['tok'],5)
            self.assertIn('${esc(k.key_alias)}',app.HTML)

if __name__=='__main__': unittest.main(verbosity=2)
