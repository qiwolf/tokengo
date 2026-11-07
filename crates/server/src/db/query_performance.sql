-- Additive, backward-readable access paths. No history is removed or reclassified.
CREATE INDEX requests_exploration_model ON requests(served_model) WHERE explored=1;
CREATE INDEX model_type_checks_model_recent ON model_type_checks(model_name,next_at DESC);
CREATE INDEX model_cooldowns_model ON model_cooldowns(model_name);
CREATE INDEX billing_ledger_channel ON billing_ledger(channel_id,id DESC,user_id,key_id,model);
CREATE INDEX billing_ledger_model ON billing_ledger(model,id DESC,user_id,key_id,channel_id);
CREATE INDEX billing_ledger_lifecycle_recent ON billing_ledger(id DESC,user_id,key_id,channel_id,model);
CREATE INDEX billing_request_prices_latest ON billing_request_prices(channel_id,model,id DESC);
-- Lifetime user portal summary reads a narrow ordered relation, not wide snapshots.
CREATE INDEX billing_ledger_portal_summary ON billing_ledger(user_id,request_id,input_tokens,output_tokens,price_id,request_price_id,debit_micro);
-- Epoch zero preserves pre-migration session proofs; expired owners get a new epoch.
ALTER TABLE protocol_sessions ADD COLUMN issued_ms INTEGER NOT NULL DEFAULT 0;

CREATE INDEX billing_ledger_user_lifecycle ON billing_ledger(user_id,id DESC,key_id,channel_id,model);
CREATE INDEX billing_ledger_key_lifecycle ON billing_ledger(key_id,id DESC,user_id,channel_id,model);
-- Statistics project only meters/dimensions, never wide error/history payloads.
-- Explicit ID ordering retains the preceding time-index tie order.
CREATE INDEX requests_stats_window ON requests(ts_ms DESC,id ASC,endpoint,status,channel_id,latency_ms,input_tokens,output_tokens,client_ip,served_model,model,input_tokens_known,output_tokens_known,usage_source);
CREATE INDEX requests_owner_stats_window ON requests(user_id,ts_ms DESC,id DESC,endpoint,status,channel_id,latency_ms,input_tokens,output_tokens,client_ip,served_model,model,input_tokens_known,output_tokens_known,usage_source);
-- Stream recent attempts in existing request/ordinal/ID order without fetching
-- wide attempt rows; Rust builds the small per-channel aggregate without sorting.
CREATE INDEX attempts_request_statistics ON attempts(request_id,ordinal,id,channel_id,outcome,duration_ms);
