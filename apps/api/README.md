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
| `DATABASE_URL` | Captures runtime role, pooled PlanetScale port **6432**, database **`captures`** |
| `MIGRATION_DATABASE_URL` | Captures migration role, direct port **5432**, same host and **`captures`** database; required at API startup |
| `WORKOS_API_KEY` | Captures environment's secret API key |
| `WORKOS_CLIENT_ID` | Matching Captures environment client ID |
| `WORKOS_WEBHOOK_SECRET` | Secret for that environment's webhook endpoint |
| `WORKOS_ISSUER` | `https://api.workos.com/`; use the exact token issuer for custom AuthKit domains |
| `CAPTURES_API_BIND` | `127.0.0.1:3001`; image uses `0.0.0.0:3001` |
| `RUST_LOG` | Optional log filter; do not enable request/body or SQL parameter logging in production |

## Shared cluster, dedicated database, separate credentials

Use a named PostgreSQL database **`captures`** inside the existing PlanetScale
`projects/main` cluster, alongside Caper's `caperchat` database. This is not a
second paid cluster. The database and its roles must be created before startup;
changing the URL path does not create a database. Databases separate application
objects/access, but still share cluster compute, storage capacity and failure modes.

Inside that database, everything including SQLx's `_sqlx_migrations` ledger stays
in the `captures` schema. The schema is an additional namespace, not a replacement
for a dedicated database. No shared `public.users` or email uniqueness constraint.
Users have an internal bigint ID and unique WorkOS ID; no public identifier.
WorkOS email and verification state are only a synchronized cache.

Example (placeholders, not credentials):

```dotenv
DATABASE_URL=postgresql://captures_app:PASSWORD@HOST:6432/captures?sslmode=verify-full
MIGRATION_DATABASE_URL=postgresql://captures_migrator:PASSWORD@HOST:5432/captures?sslmode=verify-full
```

Remote URLs require `sslmode=verify-full`, an explicit non-default database name,
and matching hosts/database paths. Migration URLs reject pooled port 6432 and
require direct port 5432 remotely; local loopback tests may use other ports and
plaintext. Only `sslmode`, `sslrootcert`, and `application_name` query parameters
are accepted so connection overrides cannot bypass those checks.

With SQLx 0.8's `runtime-tokio-rustls`, omit `sslrootcert` to use bundled public
WebPKI roots, or point it to a real CA PEM file mounted in the API container.
**Do not copy `sslrootcert=system` from a libpq URL**: SQLx treats it as a filename,
not the system trust store. Captures rejects that value instead of weakening TLS.

Use separate migration-owner and runtime roles; neither should be cluster admin.
The migration owner must be able to create/own the schema. At **every API startup**:

1. Require and validate both URLs. Never fall back to `DATABASE_URL` for DDL.
2. Run migrations through a single direct migration connection, before listening.
   Bootstrap schema creation and SQLx migrations use PostgreSQL advisory locks to
   serialize concurrent replicas. Connection/bootstrap and migration stages each
   have a five-minute timeout; failures prevent API startup.
3. Close the migration connection, then open the least-privilege runtime pool.

The API pod therefore receives **both** secrets; the web pod receives **neither**.
Closing a connection does not remove the migration secret from the API process
environment, so startup convenience carries broader credential exposure than a
separate migration runner. There is no Job dependency. Keep future migrations
backward-compatible with the old pods during rolling updates, allow adequate
startup-probe time, and avoid destructive changes during mixed-version rollouts.

The explicit migration command is still available for diagnostics/manual use:

```sh
cargo run -p captures-api -- migrate
```

This command needs only `MIGRATION_DATABASE_URL`, never the runtime URL or WorkOS
credentials. Running it against a shared environment still requires operator approval.
Provision a `captures_app` login through the database's role-management tooling,
then grant only the following with the schema owner (adapt role names):

```sql
GRANT USAGE ON SCHEMA captures TO captures_app;
GRANT SELECT, INSERT, UPDATE ON captures.users TO captures_app;
GRANT SELECT, INSERT ON captures.workos_events TO captures_app;
GRANT USAGE ON SEQUENCE captures.users_id_seq TO captures_app;
```

Audit database `CONNECT`, schema and role-inheritance privileges: naming alone is
**not** a security boundary. Restrict the new database/app roles without globally
revoking privileges needed by other apps.
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

It creates a uniquely named test database, concurrently runs startup migrations,
checks email identity isolation, stale events, disabled/deleted accounts and transactional
receipts, then drops that database. Never supply production credentials here.
Other tests use local mock WorkOS/JWKS endpoints and generated RSA test keys.

## Public API routing

- **`captur.es` → `captures-web`**: website, browser AuthKit callback, session cookie,
  and existing feedback/updater routes. The npm package remains `@captures/web`.
- **`api.captur.es/api/*` → `captures-api`**: public Rust account API and signed
  WorkOS webhooks. Native/desktop/mobile clients call this origin directly using
  WorkOS access tokens; there is no `/api/native` gateway through the website.
- The website may call Rust server-side through `CAPTURES_API_URL` (the internal
  Service address or HTTPS public origin) using the browser session's token.
  Its same-origin account endpoint is for browser cookie sessions, not a required
  gateway for other clients. No credentialed cross-origin browser CORS is enabled.

Public means internet-reachable, not anonymous: Rust validates bearer JWTs, and the
webhook endpoint validates WorkOS signatures. Keep `/health` internal. Deployment
requires a Rust image pin, Service/Ingress, Cloudflare Tunnel hostname route, DNS
and TLS for `api.captur.es`; a Kubernetes Ingress alone is insufficient. Register
the webhook URL as `https://api.captur.es/api/webhooks/workos`.

No native login UI/token storage flow is implemented or claimed tested on macOS,
Windows or Linux. These changes do not deploy the service, create the logical
database, apply PlanetScale migrations, or change WorkOS/Cloudflare configuration.
