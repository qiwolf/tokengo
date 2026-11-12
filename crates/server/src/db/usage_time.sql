-- Time-window aggregates and bounded ID selection avoid reading wide snapshots.
-- Existing lifetime/ID indexes stay in place for undated API clients.
CREATE INDEX billing_ledger_time_summary ON billing_ledger(created_ms,id,user_id,key_id,channel_id,model,request_id,input_tokens,output_tokens,price_id,request_price_id,debit_micro);
CREATE INDEX billing_ledger_owner_time_summary ON billing_ledger(user_id,created_ms,id,key_id,channel_id,model,request_id,input_tokens,output_tokens,price_id,request_price_id,debit_micro);
