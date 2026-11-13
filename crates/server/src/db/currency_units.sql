-- Preserve existing numeric values as USD; new entries default to CNY.
DROP TRIGGER billing_prices_immutable_update;
DROP TRIGGER billing_request_prices_immutable_update;
ALTER TABLE billing_prices ADD COLUMN currency TEXT NOT NULL DEFAULT 'CNY' CHECK(currency IN ('CNY','USD'));
ALTER TABLE billing_request_prices ADD COLUMN currency TEXT NOT NULL DEFAULT 'CNY' CHECK(currency IN ('CNY','USD'));
UPDATE billing_prices SET currency='USD';
UPDATE billing_request_prices SET currency='USD';
CREATE TRIGGER billing_prices_immutable_update BEFORE UPDATE ON billing_prices BEGIN SELECT RAISE(ABORT,'pricing versions are immutable'); END;
CREATE TRIGGER billing_request_prices_immutable_update BEFORE UPDATE ON billing_request_prices BEGIN SELECT RAISE(ABORT,'immutable request pricing version'); END;
ALTER TABLE billing_ledger ADD COLUMN currency TEXT NOT NULL DEFAULT 'CNY' CHECK(currency IN ('CNY','USD'));
UPDATE billing_ledger SET currency='USD';
ALTER TABLE billing_ledger ADD COLUMN pricing_unit TEXT;
ALTER TABLE billing_ledger ADD COLUMN unit_price_id INTEGER;
ALTER TABLE billing_ledger ADD COLUMN unit_price_snapshot TEXT;
ALTER TABLE billing_ledger ADD COLUMN quantity_milli INTEGER CHECK(quantity_milli IS NULL OR quantity_milli>=0);
ALTER TABLE billing_ledger ADD COLUMN bound_quantity_milli INTEGER CHECK(bound_quantity_milli IS NULL OR bound_quantity_milli>=0);
ALTER TABLE billing_budgets RENAME TO billing_budgets_old;
CREATE TABLE billing_budgets (
 scope TEXT NOT NULL CHECK(scope IN ('user','key')),scope_id INTEGER NOT NULL,
 currency TEXT NOT NULL DEFAULT 'CNY' CHECK(currency IN ('CNY','USD')),
 limit_micro INTEGER CHECK(limit_micro>=0),spent_micro INTEGER NOT NULL DEFAULT 0 CHECK(spent_micro>=0),
 reserved_micro INTEGER NOT NULL DEFAULT 0 CHECK(reserved_micro>=0),frozen INTEGER NOT NULL DEFAULT 0,
 PRIMARY KEY(scope,scope_id,currency)
);
INSERT INTO billing_budgets(scope,scope_id,currency,limit_micro,spent_micro,reserved_micro,frozen)
 SELECT scope,scope_id,'USD',limit_micro,spent_micro,reserved_micro,frozen FROM billing_budgets_old;
DROP TABLE billing_budgets_old;
CREATE TABLE billing_unit_prices (
 id INTEGER PRIMARY KEY AUTOINCREMENT,channel_id INTEGER NOT NULL,model TEXT NOT NULL,
 currency TEXT NOT NULL DEFAULT 'CNY' CHECK(currency IN ('CNY','USD')),
 unit TEXT NOT NULL CHECK(unit IN ('second','minute','hour','character','byte','image','video')),
 micro_per_unit INTEGER NOT NULL CHECK(micro_per_unit>=0),
 max_quantity_milli INTEGER NOT NULL CHECK(max_quantity_milli>=0),created_ms INTEGER NOT NULL
);
CREATE INDEX billing_unit_prices_latest ON billing_unit_prices(channel_id,model,currency,id DESC);
CREATE TRIGGER billing_unit_prices_immutable_update BEFORE UPDATE ON billing_unit_prices BEGIN SELECT RAISE(ABORT,'immutable unit pricing version'); END;
CREATE TRIGGER billing_unit_prices_immutable_delete BEFORE DELETE ON billing_unit_prices BEGIN SELECT RAISE(ABORT,'immutable unit pricing version'); END;
DROP INDEX billing_ledger_channel;
CREATE INDEX billing_ledger_channel ON billing_ledger(channel_id,id DESC,user_id,key_id,model,currency,billing_unit,unit_price_id);
DROP INDEX billing_ledger_model;
CREATE INDEX billing_ledger_model ON billing_ledger(model,id DESC,user_id,key_id,channel_id,currency,billing_unit,unit_price_id);
DROP INDEX billing_ledger_lifecycle_recent;
CREATE INDEX billing_ledger_lifecycle_recent ON billing_ledger(id DESC,user_id,key_id,channel_id,model,currency,billing_unit,unit_price_id);
DROP INDEX billing_ledger_portal_summary;
CREATE INDEX billing_ledger_portal_summary ON billing_ledger(user_id,request_id,input_tokens,output_tokens,price_id,request_price_id,debit_micro,currency,billing_unit,unit_price_id);
DROP INDEX billing_ledger_user_lifecycle;
CREATE INDEX billing_ledger_user_lifecycle ON billing_ledger(user_id,id DESC,key_id,channel_id,model,currency,billing_unit,unit_price_id);
DROP INDEX billing_ledger_key_lifecycle;
CREATE INDEX billing_ledger_key_lifecycle ON billing_ledger(key_id,id DESC,user_id,channel_id,model,currency,billing_unit,unit_price_id);
DROP INDEX billing_ledger_time_summary;
CREATE INDEX billing_ledger_time_summary ON billing_ledger(created_ms,id,user_id,key_id,channel_id,model,request_id,input_tokens,output_tokens,price_id,request_price_id,debit_micro,currency,billing_unit,unit_price_id);
DROP INDEX billing_ledger_owner_time_summary;
CREATE INDEX billing_ledger_owner_time_summary ON billing_ledger(user_id,created_ms,id,key_id,channel_id,model,request_id,input_tokens,output_tokens,price_id,request_price_id,debit_micro,currency,billing_unit,unit_price_id);
