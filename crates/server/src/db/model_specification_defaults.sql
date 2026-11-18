-- Official model defaults, reviewed 2026-11-18. An explicit input limit is an
-- editable starting reference. For a documented combined token window, the
-- input reference reserves the published maximum output within that window.
INSERT INTO model_specifications(model_id, provider_id, source_url, specification) VALUES
    ('deepseek-flash', NULL, 'https://api-docs.deepseek.com/api/list-models/',
     '{"kind":"chat","context_window":1048576,"input_usage_reference":655360,"output_usage_reference":393216,"origin":"official"}'),
    ('deepseek-v4-pro', NULL, 'https://api-docs.deepseek.com/api/list-models/',
     '{"kind":"chat","context_window":1048576,"input_usage_reference":655360,"output_usage_reference":393216,"origin":"official"}'),
    ('gpt-6-luna', NULL, 'https://developers.openai.com/api/docs/models/gpt-6-luna',
     '{"kind":"chat","context_window":1050000,"input_usage_reference":922000,"output_usage_reference":128000,"origin":"official"}'),
    ('gpt-6.1-sol', NULL, 'https://developers.openai.com/api/docs/models/gpt-6.1-sol',
     '{"kind":"chat","context_window":1050000,"input_usage_reference":922000,"output_usage_reference":128000,"origin":"official"}'),
    ('gpt-6.1-terra', NULL, 'https://developers.openai.com/api/docs/models/gpt-6.1-terra',
     '{"kind":"chat","context_window":1050000,"input_usage_reference":922000,"output_usage_reference":128000,"origin":"official"}'),
    ('claude-sonnet-4-6', NULL, 'https://platform.claude.com/docs/en/models/sonnet-4-6/overview',
     '{"kind":"chat","context_window":1000000,"input_usage_reference":872000,"output_usage_reference":128000,"origin":"official"}'),
    ('glm-5.2', NULL, 'https://docs.z.ai/guides/llm/glm-5.2',
     '{"kind":"chat","context_window":1000000,"origin":"official"}'),
    ('kimi-k2.7-code', NULL, 'https://huggingface.co/moonshotai/Kimi-K2.7-Code',
     '{"kind":"chat","context_window":262144,"origin":"official"}'),
    ('gemini-3.8-flash', NULL, 'https://aistudio.google.com/docs/models/gemini-3.8-flash',
     '{"kind":"chat","input_usage_reference":1048576,"output_usage_reference":65536,"origin":"official"}'),
    ('text-embedding-3-small', NULL, 'https://developers.openai.com/api/docs/guides/embeddings',
     '{"kind":"embeddings","context_window":8192,"output_usage_reference":0,"metrics":{"embedding_dimensions":{"value":1536,"unit":"dimension"}},"origin":"official"}'),
    ('text-embedding-3-large', NULL, 'https://developers.openai.com/api/docs/guides/embeddings',
     '{"kind":"embeddings","context_window":8192,"output_usage_reference":0,"metrics":{"embedding_dimensions":{"value":3072,"unit":"dimension"}},"origin":"official"}'),
    ('BAAI/bge-m3', NULL, 'https://huggingface.co/BAAI/bge-m3',
     '{"kind":"embeddings","context_window":8192,"output_usage_reference":0,"metrics":{"embedding_dimensions":{"value":1024,"unit":"dimension"}},"origin":"official"}');

-- Confirmed equivalent IDs use the same default record. No prefix guessing.
INSERT INTO model_specification_aliases(alias_id, model_id) VALUES
    ('deepseek-v4.1-flash', 'deepseek-flash'),
    ('deepseek/deepseek-v4.1-flash', 'deepseek-flash'),
    ('deepseek/deepseek-v4.1-flash-ultrafast', 'deepseek-flash'),
    ('deepseek/deepseek-v4-pro', 'deepseek-v4-pro'),
    ('moonshotai/Kimi-K2.7-Code', 'kimi-k2.7-code');

-- Relay assertions are scoped by exact service endpoint and advertised model ID.
-- Missing output at Command Code is left unknown, even when the lab publishes one.
INSERT INTO model_specification_relay_defaults(relay, model_id, source_url, specification) VALUES
    ('opencode_go', 'deepseek-v4.1-flash', 'https://models.opencode.ai/providers/opencode-go/',
     '{"kind":"chat","context_window":1000000,"input_usage_reference":616000,"output_usage_reference":384000,"origin":"relay_official"}'),
    ('opencode_go', 'deepseek-v4-pro', 'https://models.opencode.ai/providers/opencode-go/',
     '{"kind":"chat","context_window":1000000,"input_usage_reference":616000,"output_usage_reference":384000,"origin":"relay_official"}'),
    ('opencode_pay', 'deepseek-v4.1-flash', 'https://models.opencode.ai/providers/opencode/',
     '{"kind":"chat","context_window":1000000,"input_usage_reference":616000,"output_usage_reference":384000,"origin":"relay_official"}'),
    ('opencode_pay', 'deepseek-v4.1-flash-free', 'https://models.opencode.ai/providers/opencode/',
     '{"kind":"chat","context_window":200000,"input_usage_reference":168000,"output_usage_reference":32000,"origin":"relay_official"}'),
    ('command_code', 'deepseek/deepseek-v4.1-flash', 'https://api.commandcode.ai/provider/v1/models',
     '{"kind":"chat","context_window":1000000,"origin":"relay_official"}'),
    ('command_code', 'deepseek/deepseek-v4.1-flash-fast', 'https://api.commandcode.ai/provider/v1/models',
     '{"kind":"chat","context_window":1000000,"origin":"relay_official"}'),
    ('command_code', 'deepseek/deepseek-v4-pro', 'https://api.commandcode.ai/provider/v1/models',
     '{"kind":"chat","context_window":1000000,"origin":"relay_official"}'),
    ('command_code', 'moonshotai/Kimi-K2.7-Code', 'https://api.commandcode.ai/provider/v1/models',
     '{"kind":"chat","context_window":256000,"origin":"relay_official"}'),
    ('command_code', 'moonshotai/Kimi-K2.7-Code-Highspeed', 'https://api.commandcode.ai/provider/v1/models',
     '{"kind":"chat","context_window":262000,"origin":"relay_official"}');
