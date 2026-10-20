# Native protocol route contract

Route C adds native `POST /v1/responses` and `POST /v1/messages` beside existing Chat, embeddings, rerank and model discovery. These are authenticated gateway routes, not new administration surfaces. Sources must explicitly declare native protocol support; source names or base URLs never imply capability.

## Authentication and identity

`GatewayKey` accepts one local VKey from `Authorization: Bearer ...` or `x-api-key`. Both may appear only when their trimmed credentials agree. Repeated credentials headers and conflicting identities are rejected before routing; a malformed supplied header does not fall back to another header. Invalid credentials are not echoed in errors or runtime logs. Both representations use the same live AuthContext and credential namespace.

Incoming credentials are never provider credentials. Outbound authentication comes from the source's encrypted channel configuration: native Messages uses its configured provider credential scheme/version, while client-supplied `anthropic-version` and `anthropic-beta` cannot override source configuration. No client headers are copied wholesale to the provider. Source-specific beta headers are set by channel custom-header configuration.

The routes always populate `ForwardRequest.auth` with the verified context. Existing model/source/paid permissions, ancestor quotas, budgets and cancellation guards remain shared gateway enforcement. No protocol-specific unauthenticated forwarding shortcut is introduced. This change does not relax the root panel or delegated administrator authentication contracts.

## Request validation and preservation

Both native routes require a nonempty model string and boolean stream when supplied. Native model identifiers reject controls and names over256 bytes. Messages additionally requires a messages array and a positive integer max_tokens. Responses requires input to be a string or array; a request containing a server-state reference is allowed through this shallow parser so the central continuation guard gives the explicit unsupported-state error before forwarding.

Unknown native fields, tool definitions, tool call/result IDs, arguments and history are retained as JSON values. The routing layer changes the selected model identifier to the configured exact upstream alias; protocol-specific semantic validation belongs to the protocol module. Internal protocol flags are not inserted into the client's JSON body.

The initial provider-state profile rejects all non-null previous_response_id and conversation references via the gateway's shared guard, requiring complete input history. It is not a claim of full provider-side response-resource persistence, lookup or cancellation API support. Subsequent source-binding work must keep those restrictions until a trustworthy resource contract is implemented.

Messages authentication, shallow request-validation and locally generated gateway errors use an Anthropic-style `{type:"error",error:{type,message}}` envelope. Source-provided native responses and errors are not unconditionally converted through OpenAI envelopes. Existing routes retain their established error shape.

## Explicit session identity

The existing validated session resolver additionally accepts `x-claude-code-session-id`. All supplied explicit IDs are validated even when a higher-priority field was chosen. The resolved ID is namespaced by the authenticated credential before source-header templating. It is not an authorization identity or automatic response-resource ownership proof, and the original client header is not blindly forwarded.

## HTTP acceptance

`tests/protocol_routes.rs` starts real loopback HTTP servers for both gateway and native sources. Coverage includes native path/response shape, both credential forms, same-value dual headers, conflicts/repeats/malformed headers, native request errors, tool and unknown-field preservation, configured source version/beta, namespaced Claude session identity, VKey model/channel refusals, unknown continuation refusal, and absence of inbound VKey at the source.

Two cancellation tests cover separate boundaries: abort an HTTP request while only metadata is available before precommit, and drop an already-committed body after useful native text. Both require the request to become canceled and release admission/channel ownership so the next request can proceed. Stream implementation and its terminal/usage semantics are supplied by the shared native stream module; route tests do not replace that module's fragmented-SSE regression tests.

Source-binding, full native usage settlement and cross-protocol conversion have their own contracts and tests. This route contract does not certify deployment or production model compatibility. No real provider credential is used by the route integration fixtures.

## Verified implementation slice

The targeted loopback HTTP suite currently has nine passing tests. In addition to route/auth/body checks, it verifies source pinning across outages, independent source ownership for equal explicit session text under different VKeys, refusal after source credential rotation, and conservative opaque-history TPM preallocation. Both precommit metadata-only request abort and postcommit useful-text body drop finalize canceled requests and allow subsequent work. Evidence is under `review/route-c/evidence/protocol-routes-{red,nonstream-green,green}.log` outside the source checkout.

This is local HTTP and storage acceptance only; real provider model compatibility, deployment, and LiteLLM cutover remain separate acceptance steps.
