# Captures account API

One Rust/Axum service owns Captures account persistence and authorization.
TanStack owns the website and browser session cookie; WorkOS owns authentication.
There are no uploads, billing, public profiles, organizations, or desktop login yet.

## Endpoints

| Route | Contract |
| --- | --- |
| `GET /health` | Public liveness; not a database/WorkOS readiness probe |
| `GET /api/account/me` | WorkOS `Authorization: Bearer` token; resolves/provisions an account; returns `{email,emailVerified}` only |
| `POST /api/webhooks/workos` | Signed WorkOS `user.updated` / `user.deleted` events; 256 KiB body limit |

Missing/invalid authentication returns 401, a disabled/deleted local account 403,
and dependency failures 503. Account responses are `no-store`. Client-provided
IDs/emails never select an account. The web server forwards its session access
token server-side; browser-supplied authorization headers are ignored by its proxy.

JWT validation uses RS256, the configured issuer, required subject/session/expiry,
and client-scoped JWKS. An audience must match when present; WorkOS's default
access-token contract does not require it. Keys cache for one hour, unknown-key
refreshes are throttled, and HTTP calls have five-second timeouts. Each account
request also retrieves the current WorkOS user (no cached user/API outage fallback).
Revoked sessions can remain usable until access-token expiry; local disablement
and confirmed WorkOS user deletion deny access independently. Keep WorkOS access
tokens short-lived. This is not online session introspection.

## Runtime configuration

Set environment variables in the process environment; the Rust binary does not
load `.env` files itself. Never put secrets in the image or desktop bundle.

| Variable | Required/default |
| --- | --- |
| `DATABASE_URL` | Captures runtime role on PostgreSQL; require certificate-verified TLS for PlanetScale |
| `WORKOS_API_KEY` | Captures environment's secret API key |
| `WORKOS_CLIENT_ID` | Matching Captures environment client ID |
| `WORKOS_WEBHOOK_SECRET` | Secret for that environment's webhook endpoint |
| `WORKOS_ISSUER` | `https://api.workos.com/`; use the exact token issuer for custom AuthKit domains |
| `CAPTURES_API_BIND` | `127.0.0.1:3001`; image uses `0.0.0.0:3001` |
| `RUST_LOG` | Optional log filter; do not enable request/body or SQL parameter logging in production |

## Shared database isolation and migrations

Everything, including SQLx's `_sqlx_migrations` ledger, lives in the `captures`
schema. No `public.users`, shared identity tables, or email uniqueness constraint.
Users have an internal bigint ID and unique WorkOS ID; no public identifier.
WorkOS email and verification state are only a synchronized cache.

Use separate migration-owner and runtime roles. The migration owner must be able
to create/own the schema. Run with its connection URL explicitly:

```sh
cargo run -p captures-api -- migrate
```

This command needs only `DATABASE_URL`. Server startup never runs migrations.
Run each migration as an approved deployment step, not in every replica.
Provision a `captures_app` login through the database's role-management tooling,
then grant only the following with the schema owner (adapt role names):

```sql
GRANT USAGE ON SCHEMA captures TO captures_app;
GRANT SELECT, INSERT, UPDATE ON captures.users TO captures_app;
GRANT SELECT, INSERT ON captures.workos_events TO captures_app;
GRANT USAGE ON SEQUENCE captures.users_id_seq TO captures_app;
```

Audit existing shared-database privileges: schema naming is **not** a security
boundary if a role also inherits broad privileges or other schemas grant access
to `PUBLIC`. Restrict roles without revoking privileges needed by other apps.
Use a disposable database for local development, not the shared production branch.

### Lifecycle

The first authenticated account lookup atomically upserts by WorkOS ID. Email
matches never merge accounts. `disabled_at` is operator-controlled and synchronization
never clears it. Webhook receipts and account changes commit together before 200;
duplicates and stale updates do not overwrite newer data. Updates alone do not
create local accounts. Deletion creates a tombstone even before first login, clears
cached email, and cannot be undone by a delayed callback/update. A confirmed user
404 from WorkOS takes the same path. Other WorkOS failures do not delete accounts.

Tombstones retain the internal/WorkOS IDs to prevent resurrection; this is not a
complete self-service account deletion/retention system. That policy and UI are
future work. Configure only `user.updated` and `user.deleted` webhook events and
monitor failed deliveries. Retry failed events through WorkOS; a deployment outage
beyond its retry window requires reconciliation before reopening account access.
Event receipts contain no full user payloads; an automated pruning policy is not
included yet.

## Build and test

```sh
cargo test -p captures-api
cargo clippy -p captures-api --all-targets -- -D warnings
cargo build --locked --release -p captures-api
docker build -f apps/api/Dockerfile -t captures-api .
```

Run the PostgreSQL integration test explicitly against a **disposable** server
whose test role can create databases:

```sh
TEST_DATABASE_URL=postgres://captures_test@127.0.0.1:55432/postgres \
  cargo test -p captures-api -- --include-ignored
```

It creates a uniquely named test database, applies migrations, checks concurrency,
email identity isolation, stale events, disabled/deleted accounts and transactional
receipts, then drops that database. Never supply production credentials here.
Other tests use local mock WorkOS/JWKS endpoints and generated RSA test keys.

Deploy the API separately from the website. Route the public webhook path to Rust;
the website's `CAPTURES_API_URL` points at its private service address. Do not expose
the Rust liveness endpoint as a public account API. Future native clients will need
a public authenticated API ingress; no native token-storage/login flow is implemented
or claimed tested on macOS, Windows, or Linux. This PR does not deploy the service,
apply PlanetScale migrations, or change WorkOS configuration.
