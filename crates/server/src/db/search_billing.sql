CREATE TABLE billing_request_prices (
 id INTEGER PRIMARY KEY AUTOINCREMENT, channel_id INTEGER NOT NULL, model TEXT NOT NULL,
 micro_per_call INTEGER NOT NULL CHECK(micro_per_call>=0), created_ms INTEGER NOT NULL
);
CREATE TRIGGER billing_request_prices_immutable_update BEFORE UPDATE ON billing_request_prices BEGIN SELECT RAISE(ABORT,'immutable request pricing version'); END;
CREATE TRIGGER billing_request_prices_immutable_delete BEFORE DELETE ON billing_request_prices BEGIN SELECT RAISE(ABORT,'immutable request pricing version'); END;
ALTER TABLE billing_ledger ADD COLUMN billing_unit TEXT NOT NULL DEFAULT 'token' CHECK(billing_unit IN ('token','request'));
ALTER TABLE billing_ledger ADD COLUMN request_units INTEGER CHECK(request_units IS NULL OR request_units IN (0,1));
ALTER TABLE billing_ledger ADD COLUMN request_price_id INTEGER;
ALTER TABLE billing_ledger ADD COLUMN request_price_snapshot TEXT;
