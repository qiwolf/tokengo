# Rust relay foundation implementation

Goal: synchronize the fork, fix bounded streaming error response handling, and deliver the first usable logical-routing backend from the reviewed roadmap.

Base: upstream v0.34.0 / 3c1460a. The existing reviewed architecture and user authorization govern this implementation. Rust/Axum/SQLite/Leptos remain in use; no production deployment or provider credentials are needed.

## Deliverables

- [x] Fast-forward local main and GitHub fork main to upstream, then work on codex/relay-foundation.
- [x] Bound non-2xx streaming response reads by remaining request/attempt deadlines, idle time, and 64 KiB. Preserve finite errors; fail over before output on transport failure. Add real HTTP regression tests with red/green evidence.
- [x] Restore logical-model to upstream-model mappings through the authenticated channel API. Validate normalized logical identities and preserve exact upstream identifiers. Test source-specific diagnostics and existing admission checks.
- [x] Add authenticated per-model routing policy API: ordered channel allowlist, retry override, recovery interval, probe control, and exclusion from auto. Unconfigured names inherit current behavior. Store policy in existing settings, preserving prior migrations.
- [x] Verify the representative three-source chain and isolated diagnostic aliases using controlled HTTP. No real provider account availability is inferred.
- [x] Add GitHub Actions for the supported Rust version, native workspace tests, and WASM check.
- [x] Review changes, run proportionate integrated checks, commit separately, push a reviewable development branch, and record remaining roadmap work.

## Interfaces and ownership

Error-response worker owns gateway.rs and tests/error_response.rs. Alias worker owns routes/admin.rs and tests/logical_models.rs plus the superseded alias rejection test. CI worker owns .github/workflows/ci.yml and docs/ci.md. Primary owns db/route_policy.rs, routes/model_routing.rs, route selection integration, and policy tests. All work is on the same branch; only the primary commits.

Routing API: GET/PUT/DELETE `/api/model-routing?model=...` with PanelAuth. A PUT is a complete replacement of optional overrides: channel_ids, retry_times, reprobe_interval_ms, probe_enabled, include_in_auto. Empty JSON clears overrides; DELETE restores defaults. Invalid or unconfigured targets return 400/404 without mutation. Channel lists are explicit allowlists and order only already-declared mappings; disabled and capability gates still apply. Policy changes reset that model's persisted active route for future requests. Existing in-flight requests use their captured plan; full policy-generation cancellation is deferred.

## Verification and boundaries

Tests must cover stalled/oversized errors, deadlines and permit release; alias validation and actual upstream names; ordered/limited route targets, request retries, disabled models/channels, auto exclusion, authentication, failed-write preservation, and restart. Run the native workspace suite and WASM check after integration, then verify actual GitHub CI where available. Keep existing warnings separate from regressions.

This is the first roadmap A batch, not completion of the entire relay. Users/vkey scopes and budgets, durable financial ledger, native Responses/Messages conversion, subscription OAuth, multimedia adapters, browser UI authoring, and production migration remain subsequent deliverables. Logical aliases do not yet share all model-specific permission/cooldown state across the same physical target; no lower-trust user interface is exposed in this batch.
