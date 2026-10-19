# Logical model routing (fork development branch)

This fork builds on upstream v0.34.0. The first roadmap batch adds bounded error responses, administrator-configured logical mappings and per-model route policy, stable session adapters, and CI. It does not yet add users, vkey scopes/budgets, native Responses/Messages protocols, subscription OAuth, or production migration.

## Upstream names and logical names

An authenticated administrator can POST or PUT `/api/channels` with mappings:

```json
{
  "name": "provider-a",
  "base_url": "https://provider-a.example/v1",
  "models": [
    {"model": "deepseek-v4.1-flash", "upstream": "deepseek-v4.1-flash"},
    {"model": "diag/provider-a/deepseek-v4.1-flash", "upstream": "deepseek-v4.1-flash"}
  ]
}
```

Two more channels can declare the same logical name with their own upstream identifiers, such as `deepseek/deepseek-v4.1-flash` and `deepseek-flash`. These names illustrate the reviewed route; the real providers and account permissions still require validation. Save the IDs returned by channel creation instead of assuming particular numeric IDs.

Logical names are trimmed and compared using the existing gateway normalization. Empty/reserved `auto`, control characters and normalized duplicates within one channel are rejected before mutation. Upstream identifiers are only trimmed; case, punctuation, provider prefix and version remain intact. Empty upstream means the logical name itself is sent. Configuration is still administrator-only.

Use separate diagnostic names such as `diag/provider-a/...`. A suffix like `(provider-a)` or `Free` is removed by existing normalization and cannot reliably separate routes. A model enable switch remains scoped to its logical name; it does not disable every other alias of a physical model.

## Per-model policy API

Use the existing panel session token, not a gateway inference key:

```sh
curl -X PUT "$BASE/api/model-routing?model=deepseek-v4.1-flash" \
  -H "Authorization: Bearer $ADMIN_TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{"channel_ids":[1,2,3],"retry_times":0,"reprobe_interval_ms":60000,"probe_enabled":false}'
```

Replace `BASE`, the token and numeric channel IDs with your own values. No real secrets belong in this document or Git.

GET `/api/model-routing?model=...` returns `{model,policy}`. PUT completely replaces that model's optional overrides. Empty JSON or DELETE restores inheritance. Query values containing spaces or other reserved characters must be URL encoded. Missing models are rejected on PUT; a stale policy can still be removed. Existing settings and database migrations are preserved.

| Field | Meaning |
| --- | --- |
| `channel_ids` | Ordered **allowlist** of 1–256 distinct positive channel IDs already declaring this logical model. Unlisted channels are never used. Omit to inherit channel order. |
| `retry_times` | 0–2 retries after the first attempt. Recovery trials remain a single attempt. Applies to the logical candidate even when selected by `auto`. Omit to inherit the global configuration. |
| `reprobe_interval_ms` | 0–86400000 ms before checking the preferred source again after a switch. Omit to inherit. |
| `probe_enabled` | Override scheduled recovery probes. Explicit false also prevents automatic type/vision inference requests for this name, including later attempts of a detector job. Metadata-only GET discovery remains separate. |
| `include_in_auto` | False excludes this logical name from auto selection while preserving direct access. Omit to include as before. |

Invalid fields, unknown/undeclared targets, duplicate IDs and invalid values fail without replacing the previous policy. The policy and reset active-route state are saved in one transaction; future requests use the new plan. Existing in-flight requests can finish their captured plan. Full policy-generation cancellation is not part of this batch.

An older upstream binary does not understand these additional settings. Rolling back the executable alone would stop enforcing the new route policies; use a reviewed, matching configuration/database backup when reverting versions. These policies are not a cross-version user-authorization boundary.

Disabled channels/models and visual capability checks still apply; a policy cannot admit an undeclared or disabled target. The model list uses the same target allowlist. A diagnostic route should declare only its source and use a one-ID policy with `retry_times:0`, `probe_enabled:false`, and `include_in_auto:false`.

## Three-source acceptance contract

Controlled HTTP tests cover A429 → B400 with structured `insufficient_balance` → C200, preserving each exact upstream model name. A per-route zero-retry override keeps global retry settings unchanged. Source-specific diagnostics do not switch sources, and are excluded from auto when configured as above.

Unstructured 400 messages such as `insufficient credits` are not globally reclassified as provider balance errors. Provider-specific error adapters must verify real response shapes before allowing paid fallback. No actual provider quota or subscription availability is inferred from these controlled tests.

Account-wide faults already suspend a channel. Some model-level permission/cooldown state is still scoped by logical name, so aliases of one physical target do not yet share every state. This must be addressed before a future user-scoped authorization layer depends on physical target suspension. This batch does not provide vkey model permissions or paid-fallback authorization.

## Stable conversation identity

The adapter accepts these sources in order:

1. Request headers: `x-opencode-session`, `session_id`, `x-session-id`, `x-conversation-id`.
2. Top-level body `session_id`, then `conversation_id`.
3. `metadata.conversation_id`, then `metadata.session_id`.

Every supplied explicit ID must be a string, nonblank, at most 256 UTF-8 bytes and free of control characters, including lower-priority fields. Invalid IDs receive 400 before any upstream request. Explicit identity is hashed within the authenticated gateway key's namespace and only reaches the upstream through its configured `{{session_id}}` header template; arbitrary inbound authorization is not forwarded.

Without explicit identity the existing first-user-content heuristic remains for complete-history clients. If a route uses a session template and the request has no original user message, an explicit ID is required before any source is called. Supply a real conversation ID when distinct conversations under one key start with the same text; the heuristic cannot distinguish them.

## Streaming error response bounds

Non-2xx error bodies are read before committing a client response. Reads have the remaining attempt and request deadline, an idle deadline, and a decompressed 64 KiB cap. A hung, broken or oversized body becomes a channel fault and may switch to an allowed source if budget remains. No complete response is fabricated from a partial body. Complete finite errors preserve upstream status, body, Content-Type and Retry-After.

Successful SSE generation retains the existing idle-timeout behavior; this fix does not impose a short total timeout on a healthy stream. Already-emitted output is never silently restarted on another provider.

## Verification and delivery boundary

Run `cargo test --locked --workspace` and `cargo check --locked -p tokengo-web --target wasm32-unknown-unknown`. New real HTTP suites cover bounded errors, mappings, route policy, sessions and disabled automatic probes. GitHub Actions runs native tests and WASM check using Rust 1.98.0. There is no automatic production deployment.

This batch exposes routing controls through the authenticated API. A dedicated policy editor and further UI work remain separate. Before using the existing panel to edit a channel, keep its logical mappings intact; the API rejects ambiguous duplicate logical names rather than silently merging them.
