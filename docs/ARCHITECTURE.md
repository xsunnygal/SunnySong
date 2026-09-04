# Architecture

## Dependency rule

Dependencies point inward:

```text
Svelte UI → Tauri commands → application use cases → domain
                         ↘ adapters implement ports ↗
```

The domain does not know about Tauri, SQLite, HTTP, YouTube, Media3, or operating systems.

## Rust workspace

- `solmusic-domain`: stable entities and invariants with no infrastructure dependencies.
- `solmusic-application`: use cases, ports, orchestration, and policy. It may depend on domain.
- Future adapter crates: YouTube provider, SQLite storage, playback, media integration, and sync.
- `src-tauri`: composition root and transport adapter only. Commands translate DTOs and call use cases; they do not contain business rules.

New crates are added only when a real boundary exists. Internal modules are preferred over tiny crates with no isolation benefit.

## Frontend

- `src/lib/features`: UI grouped by user capability, not technical file type.
- `src/lib/components`: genuinely shared presentation components.
- `src/lib/api`: the only direct Tauri invocation boundary.
- Svelte stores represent UI projections, not a second copy of backend business logic.

## Playback ownership

Rust owns queue policy, normalized state, session tracking, and recommendation decisions. Native adapters own decoding, buffering, audio focus, background execution, and OS media sessions. Events synchronize native playback facts back into the application core.

## Provider boundary

Raw YouTube/InnerTube structures never escape the provider adapter. Playback URLs are ephemeral values and are never persisted as song metadata. Provider failures must be typed so the UI can distinguish unavailable content, expired resolution, throttling, and connectivity errors.

## Data

SQLite access is behind repositories and migrations are append-only once released. Playback position stays in memory; durable writes summarize completed/interrupted sessions. Recent events are bounded while lifetime aggregates remain compact.

## Recommendation separation

`QuickPicks` and `NextUp` are separate use cases with separate candidate generation and scoring. Shared low-level utilities are allowed, but neither recommender calls the other.

## Cross-cutting rules

- Configuration owns tunable weights and thresholds.
- Cancellation/timeouts propagate across network and playback operations.
- Public boundaries use typed errors and structured diagnostics.
- Platform conditionals stay in adapters, not domain or application policy.
- Tests concentrate on domain invariants, scoring, queue rules, session summaries, migrations, and adapter contracts.
