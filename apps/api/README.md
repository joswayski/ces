# Captures account API foundation

The Rust/Axum service retains a provider-independent PostgreSQL users table and
startup migrations for future account development. Authentication, account
provisioning, and lifecycle webhooks are not available. There are no uploads,
billing, public profiles, organizations, or desktop login.

## Endpoints

| Route | Contract |
| --- | --- |
| `GET /health` | Public liveness; not a database readiness probe |
| `GET /api/account/me` | Always 503 with `Cache-Control: no-store`; credentials do not enable access |

The website shows an unavailable notice at `/account`. It does not create sessions,
forward access tokens, or connect to the Rust API. Local desktop capture is unaffected.

## Runtime configuration

Set variables in the process environment; the Rust binary does not load `.env`
files itself. Never put secrets in the image or desktop bundle.

| Variable | Required/default |
| --- | --- |
| `DATABASE_URL` | Captures runtime role, pooled PlanetScale port **6432**, database **`captures`** |
| `MIGRATION_DATABASE_URL` | Captures migration role, direct port **5432**, same host and database; required at startup |
| `CAPTURES_API_BIND` | `127.0.0.1:3001`; image uses `0.0.0.0:3001` |
| `RUST_LOG` | Optional filter; do not enable request/body or SQL parameter logging in production |

The Node website receives neither database URL nor account secrets.

## Shared cluster, dedicated database, separate credentials

Use the named PostgreSQL database **`captures`** in the existing PlanetScale
`projects/main` cluster, alongside Caper's `caperchat` database. This is not a
second paid cluster. Create the database and roles before startup; changing a URL
path does not create a database. Database separation still shares cluster compute,
storage capacity, and failure modes.

All application objects, including SQLx's `_sqlx_migrations` ledger, stay in the
`captures` schema. Users retain an internal bigint ID, nullable email, verification
state (false by default), creation/update timestamps, and disabled/deleted timestamps.
Deleted rows must have no email. There is no public identifier or email uniqueness
constraint. No account writes are currently exposed by the service.

Runtime queries must explicitly qualify the schema. Runtime connections must not
send a `search_path` startup option, which PlanetScale's pooler rejects. Only the
direct migration connection sets it, keeping the migration ledger isolated.

```dotenv
DATABASE_URL=postgresql://captures_app:PASSWORD@HOST:6432/captures?sslmode=verify-full
MIGRATION_DATABASE_URL=postgresql://captures_migrator:PASSWORD@HOST:5432/captures?sslmode=verify-full
```

Remote URLs require `sslmode=verify-full`, an explicit non-default database name,
and matching hosts/database paths. Migration URLs reject pooled port 6432 and
require direct port 5432 remotely; loopback tests may use other ports and plaintext.
Only `sslmode`, `sslrootcert`, and `application_name` URL parameters are accepted.
With SQLx 0.8's `runtime-tokio-rustls`, omit `sslrootcert` for bundled public roots,
or use a real CA PEM file mounted in the API container. Do not use
`sslrootcert=system`: SQLx treats it as a filename.

Use separate migration-owner and runtime roles; neither should be cluster admin.
The migration owner must be able to create/own the schema. At every API startup:

1. Validate both URLs; never fall back to runtime credentials for DDL.
2. Run migrations through one direct connection before listening. Advisory locks
   serialize concurrent schema bootstrap and SQLx migrations. Bootstrap and
   migration stages each have a five-minute timeout; failures prevent startup.
3. Close the migration connection, then open the runtime pool.

The API receives both secrets. Closing the migration connection does not remove
its secret from the process environment. The explicit diagnostic command needs
only `MIGRATION_DATABASE_URL`:

```sh
cargo run -p captures-api -- migrate
```

Grant the runtime role access with the schema owner (adapt role names):

```sql
GRANT USAGE ON SCHEMA captures TO captures_app;
GRANT SELECT, INSERT, UPDATE ON captures.users TO captures_app;
GRANT USAGE ON SEQUENCE captures.users_id_seq TO captures_app;
```

Audit database `CONNECT`, schema, and role-inheritance privileges without globally
revoking access needed by other apps. Use a disposable database for development.

## One-time reset of the unused account schema

The initial migration was rewritten deliberately, not extended with an upgrade
migration. Existing ledgers have a different checksum. Do not roll out the new
API against the old schema or merely change the ledger checksum.

For the approved empty installation, stop the old API and prevent reconciliation
from restarting it before resetting. From the infrastructure checkout, suspend
its application reconciliation and scale down:

```sh
flux suspend kustomization production-apps -n flux-system
kubectl -n default scale deployment/captures-api --replicas=0
kubectl -n default wait --for=delete pod -l app.kubernetes.io/name=captures-api --timeout=5m
```

From this checkout, set `MIGRATION_DATABASE_URL` securely to the direct **captures**
database as the migration owner, then run the scoped reset and new migration.
This destroys only the `captures` schema and its unused account data/ledger:

```sh
psql "$MIGRATION_DATABASE_URL" -v ON_ERROR_STOP=1 <<'SQL'
DO $$ BEGIN
  IF current_database() <> 'captures' THEN
    RAISE EXCEPTION 'Expected captures database';
  END IF;
END $$;
DROP SCHEMA captures CASCADE;
SQL
cargo run -p captures-api -- migrate
```

Reapply the runtime grants above because recreated objects lose their grants.
Verify the new API image pin in infrastructure `main` before resuming reconciliation; never restart the old
API against the reset schema. Resume and verify from the infrastructure checkout:

```sh
flux resume kustomization production-apps -n flux-system
flux reconcile kustomization production-apps -n flux-system --with-source
kubectl -n default rollout status deployment/captures-api --timeout=15m
kubectl -n default exec deployment/captures-web -- node -e \
  'fetch("http://captures-api/api/account/me").then(r=>{console.log(r.status);if(r.status!==503)process.exit(1)})'
```

The account endpoint must return 503. Coordinate this reset with Caper's separate
`caperchat` reset and deploy both provider-free websites before removing old secret
projections. No reset or deployment happens merely by publishing an image.

## Build and test

```sh
cargo test -p captures-api
cargo clippy -p captures-api --all-targets -- -D warnings
cargo build --locked --release -p captures-api
docker build -f apps/api/Dockerfile -t captures-api .
TEST_DATABASE_URL=postgres://captures_test@127.0.0.1:55432/postgres \
  cargo test -p captures-api -- --include-ignored
```

The ignored PostgreSQL test creates a unique disposable database, concurrently
runs startup migrations, verifies schema/ledger isolation, checks the generic
users columns and unverified defaults, and drops that database. The test role
must be able to create databases. Never supply production credentials.

## Image publication and routing

The `AWS API image` workflow builds a non-root `linux/arm64` API image on PRs and
merges. A disposable PostgreSQL container checks migrations, restart, health,
account unavailability, and closure of the migration connection.

On `main`, the tested image is published to `production/captures` in ECR as
`api-<full Git SHA>`. Website images retain unprefixed SHA tags. Both use the
existing OIDC publisher role. Reruns preserve immutable tags; deploy by tag and
digest, never `latest`.

Successful publication sends a **Captures API image is ready** notification with
an exact SHA/digest through `DEPLOY_NOTIFICATION_WEBHOOK_URL`. The **Deploy
Captures API** button uses Godis's `captures-api` route to
`deploy-captures-api.yml`; **Open GitHub** is the workflow fallback. Missing webhook
configuration skips notification. A failed notification can be retried without
overwriting the image. Publication does not deploy or configure secrets.

- `captur.es` routes to `captures-web`: website, feedback, and updater routes.
- `api.captur.es/api/*` routes to `captures-api`: unavailable account endpoint.
- Keep Rust `/health` internal. No native login or token storage is implemented or
  claimed tested on macOS, Windows, or Linux.

Infrastructure still owns Service/Ingress, Tunnel routing, DNS, TLS, and deployment
pins. No cluster, production database, or cloud configuration is changed by tests.
