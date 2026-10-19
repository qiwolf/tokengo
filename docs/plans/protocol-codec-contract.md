# Protocol codec contract

The codec distinguishes `chat`, `responses`, and `messages`. It does not perform authentication, routing, retries, billing, persistence, or networking. `Protocol::path()` returns a path relative to an upstream base URL, normally ending in `/v1`.

## Public API and ownership

- `convert_request(from, to, body)` converts a bounded, stateless text/function-tool request.
- `convert_response(from, to, body)` converts a non-streaming response; `from` is the upstream protocol and `to` is the client protocol.
- `validate_response(protocol, body)` checks the native response envelope without filtering provider fields.
- `has_image(protocol, body)` recursively inspects Messages/Chat message content and Responses input, including images nested inside native tool-result `content`/`output`; function-argument strings are not treated as image blocks.
- `protocol::sse` owns streaming frame parsing and event translation independently.

Same-protocol conversion returns an exact JSON-value clone, including unknown fields, reasoning signatures, opaque state and native image blocks. Envelope validation is separate: native empty payloads fail validation, as do incomplete Responses envelopes; native legacy `function_call`, multimodal text blocks, refusals, reasoning and opaque nonempty provider output remain intact. The response converter can represent maximum-output truncation, but that does not override the gateway's stricter native-success validation. The gateway must retain responsibility for current authorization, per-attempt token bounds, financial reservation/settlement, cancellation, and the point at which downstream output becomes irreversible.

## Supported conversion subset

All six different-protocol directions support text messages, system instructions where their position/priority is representable, function tools, explicit tool choice, parallel function calls, function-call IDs and JSON arguments, and tool results in a full conversation history. Responses `call_id`, Chat `tool_call_id`/call `id`, and Messages `tool_use.id`/`tool_use_id` are carried through. Response item IDs generated for a converted Responses response are stable derived identifiers, separate from call IDs.

Sampling parameters are range-checked for the target. Output caps map among `max_tokens`, `max_completion_tokens` and `max_output_tokens`; absent caps become 4096 only when targeting Messages, whose native request requires a cap. Converted requests targeting Responses explicitly set `store:false`. The gateway must calculate bounds from the actual converted request before reserving money. A non-Chat endpoint must never imply zero output reservation.

Non-streaming termination maps normal completion, tool calls and maximum-output truncation. Reported zero usage stays zero; absent usage remains absent. Messages usage does not receive an invented `total_tokens` field. Responses output messages/function calls receive item IDs and statuses; normal response configuration echoes are accepted but unsupported state is rejected.

## Explicit rejection boundaries

Different-protocol conversion rejects unknown top-level or content fields rather than silently deleting them. Native pass-through remains the path for:

- image conversion, thinking/reasoning signatures, encrypted state and hosted/server tools;
- Responses `previous_response_id`/conversation state and explicitly requested storage;
- structured-output formats, nonempty annotations/logprobs and provider-specific blocks;
- multiple Chat choices, developer-priority instructions targeting Messages, mid-history system instructions that would move position, or text interleaved after assistant tool use;
- failed/paused/refusal/unsupported stop semantics and error tool-result semantics that the target cannot faithfully express;
- nonzero Messages cache-read/cache-creation usage in a converted non-streaming response, until a target representation and pricing treatment are explicitly implemented.

Custom stop sequences can map Chat↔Messages requests but have no supported Responses mapping. A stop-sequence-specific Messages response is rejected rather than pretending it was an ordinary end of turn. JSON tool arguments must be complete objects when targeting Messages; malformed/truncated JSON is rejected.

These restrictions deliberately describe an initial subset. They are not proof that an entire Claude Code or Codex workflow has been accepted: that requires the gateway's real HTTP integration and actual client tests, including streaming, tool continuation, cancellation, auth, and accounting.

## Verification

`protocol::tests` starts with native cloning plus four observed failing conversion/validation tests against stubs, recorded in `review/implementation/evidence/protocol-codec-red.log`. Additional boundary tests cover stateless Responses output, stable item IDs, zero usage, truncation and explicit rejection. Tests use representative native JSON structures and do not call external providers or contain credentials.

Reference schemas: [OpenAI Responses create](https://developers.openai.com/api/reference/resources/responses/methods/create), [Claude Messages reference](https://platform.claude.com/docs/en/api/typescript/messages), [Claude tool-call lifecycle](https://platform.claude.com/docs/en/agents-and-tools/tool-use/handle-tool-calls). Unknown additions from future API revisions remain native-only until explicitly mapped.


## Streaming usage and conversion boundaries

Native streams are forwarded byte-for-byte. Financial usage comes from an independent source-protocol `Observer`, whose missing input/output values remain `None`; Messages input accounting includes input, cache-read, and cache-creation tokens with checked arithmetic and cumulative maxima.

For conversion to Messages, `message_start` and terminal `message_delta` include schema-required numeric `usage.input_tokens` and `usage.output_tokens`. Missing source values use wire-format zero placeholders for client compatibility, and known final values replace them. These placeholders are not financial evidence: never derive billing known flags or charges by observing the generated target stream.

Text streams immediately. To maintain sequential Messages content-block lifecycles, converted parallel tool calls buffer their arguments until source termination and then emit complete start/delta/stop blocks. Text after a tool block, reasoning/refusal or opaque provider blocks, non-text content, and nonzero Messages cache buckets are explicitly rejected for incompatible conversion rather than silently discarded. Native forwarding retains these provider fields. Each SSE frame is limited to 1 MiB; converted accumulated content is bounded at 16 MiB.
