"""Read-only schema47 acceptance; derive objects from the verified release SQL, not old schema45 lists."""
from pathlib import Path
import hashlib,json,re,sqlite3

def check(database,release):
    database=Path(database);release=Path(release);manifest=json.loads((release/'manifest.json').read_text())
    assert manifest['schema_version']==47
    statements=[];migration_hashes={}
    for version in ['43','44','45','46','47']:
        record=manifest['migrations'][version];path=release/record['archive_path'];raw=path.read_bytes()
        assert hashlib.sha256(raw).hexdigest()==record['sha256'];migration_hashes[version]=record['sha256'];statements.append(raw.decode())
    with sqlite3.connect('file:'+str(database)+'?mode=ro',uri=True) as c:
        assert c.execute('PRAGMA integrity_check').fetchall()==[('ok',)]
        assert c.execute('PRAGMA foreign_key_check').fetchall()==[]
        assert c.execute('SELECT max(version) FROM schema_migrations').fetchone()[0]==47
        columns={table:{r[1]:r for r in c.execute('PRAGMA table_info('+table+')')} for table in ['billing_prices','billing_request_prices','billing_ledger','billing_budgets','billing_unit_prices']}
        for table in columns:
            row=columns[table]['currency'];assert row[2]=='TEXT' and row[3]==1 and row[4]=="'CNY'"
        assert {'pricing_unit','unit_price_id','unit_price_snapshot','quantity_milli','bound_quantity_milli'}<=set(columns['billing_ledger'])
        assert {r[1]:r[5] for r in columns['billing_budgets'].values() if r[5]}=={'scope':1,'scope_id':2,'currency':3}
        assert {'unit','micro_per_unit','max_quantity_milli','currency'}<=set(columns['billing_unit_prices'])
        # Process migration order: 47 intentionally replaces indexes first added
        # in 46. Only the last CREATE for each name is the final expected object.
        expected={}
        for sql in statements:
            for pattern in [r'CREATE INDEX\s+(\w+)\s+[^;]+;',r'CREATE TRIGGER\s+(\w+)\b.*?\bEND;']:
                for match in re.finditer(pattern,sql,re.I|re.S):expected[match[1]]=match[0]
        normalize=lambda value:' '.join(value.strip().rstrip(';').split())
        actual=dict(c.execute("SELECT name,sql FROM sqlite_master WHERE type IN ('index','trigger')"))
        for name,sql in expected.items():assert name in actual and normalize(sql)==normalize(actual[name]),name
        assert 'billing_unit_prices_immutable_update' in actual and 'billing_unit_prices_immutable_delete' in actual
        return {'schema_version':47,'integrity':'ok','foreign_keys':'ok','verified_migration_hashes':migration_hashes,'verified_index_trigger_count':len(expected),'currency_unit_columns':True,'budget_composite_key':True}
