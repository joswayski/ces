CREATE SCHEMA IF NOT EXISTS captures;

CREATE TABLE captures.users (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    workos_user_id text NOT NULL UNIQUE,
    email text,
    email_verified boolean NOT NULL DEFAULT false,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    workos_updated_at timestamptz,
    disabled_at timestamptz,
    deleted_at timestamptz,
    CHECK (deleted_at IS NULL OR email IS NULL)
);

CREATE TABLE captures.workos_events (
    event_id text PRIMARY KEY,
    event_type text NOT NULL,
    processed_at timestamptz NOT NULL DEFAULT now()
);
