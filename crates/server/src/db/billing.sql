CREATE TABLE billing_prices (
 id INTEGER PRIMARY KEY AUTOINCREMENT,
 channel_id INTEGER NOT NULL, model TEXT NOT NULL,
 input_micro_per_million INTEGER NOT NULL CHECK(input_micro_per_million>=0),
 output_micro_per_million INTEGER NOT NULL CHECK(output_micro_per_million>=0),
 max_input_tokens INTEGER NOT NULL CHECK(max_input_tokens>=0),
 max_output_tokens INTEGER NOT NULL CHECK(max_output_tokens>=0),
 created_ms INTEGER NOT NULL
);
CREATE INDEX billing_prices_latest ON billing_prices(channel_id,model,id DESC);
CREATE TRIGGER billing_prices_immutable_update BEFORE UPDATE ON billing_prices BEGIN SELECT RAISE(ABORT,'pricing versions are immutable'); END;
CREATE TRIGGER billing_prices_immutable_delete BEFORE DELETE ON billing_prices BEGIN SELECT RAISE(ABORT,'pricing versions are immutable'); END;
CREATE TABLE billing_budgets (
 scope TEXT NOT NULL CHECK(scope IN ('user','key')),scope_id INTEGER NOT NULL,
 limit_micro INTEGER CHECK(limit_micro>=0),spent_micro INTEGER NOT NULL DEFAULT 0 CHECK(spent_micro>=0),
 reserved_micro INTEGER NOT NULL DEFAULT 0 CHECK(reserved_micro>=0),frozen INTEGER NOT NULL DEFAULT 0,
 PRIMARY KEY(scope,scope_id)
);
CREATE TABLE billing_ledger (
 id INTEGER PRIMARY KEY AUTOINCREMENT,attempt_id TEXT NOT NULL UNIQUE,
 user_id INTEGER NOT NULL,key_id INTEGER NOT NULL,request_id INTEGER NOT NULL,
 channel_id INTEGER NOT NULL,model TEXT NOT NULL,price_id INTEGER,price_snapshot TEXT,
 input_bound INTEGER NOT NULL,output_bound INTEGER NOT NULL,
 reserved_micro INTEGER NOT NULL CHECK(reserved_micro>=0),debit_micro INTEGER CHECK(debit_micro>=0),
 observed_cost_micro INTEGER,input_tokens INTEGER,output_tokens INTEGER,
 usage_certainty TEXT NOT NULL DEFAULT 'pending',outcome TEXT,status TEXT NOT NULL DEFAULT 'reserved',overrun INTEGER NOT NULL DEFAULT 0,
 created_ms INTEGER NOT NULL,settled_ms INTEGER
);
CREATE INDEX billing_ledger_user ON billing_ledger(user_id,id DESC);
CREATE INDEX billing_ledger_key ON billing_ledger(key_id,id DESC);
CREATE INDEX billing_ledger_pending ON billing_ledger(status);
CREATE TABLE billing_audit (
 id INTEGER PRIMARY KEY AUTOINCREMENT,event TEXT NOT NULL,actor TEXT NOT NULL,object TEXT NOT NULL,detail TEXT NOT NULL,created_ms INTEGER NOT NULL
);
CREATE TABLE billing_allocations (
 reservation_id INTEGER NOT NULL REFERENCES billing_ledger(id),scope TEXT NOT NULL,scope_id INTEGER NOT NULL,reserved_micro INTEGER NOT NULL,
 PRIMARY KEY(reservation_id,scope,scope_id)
);
