use std::process::{Command, Output};

fn api() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_captures-api"));
    command
        .env_remove("DATABASE_URL")
        .env_remove("MIGRATION_DATABASE_URL")
        .env("CAPTURES_API_BIND", "127.0.0.1:0");
    command
}

fn failure(output: Output, expected: &str) {
    assert_eq!(output.status.code(), Some(2));
    let error = String::from_utf8(output.stderr).unwrap();
    assert!(error.contains(expected), "unexpected error: {error}");
    assert!(!error.contains("private-password"));
}

#[test]
fn migration_command_never_falls_back_to_runtime_credentials() {
    failure(
        api()
            .arg("migrate")
            .env(
                "DATABASE_URL",
                "postgres://app:private-password@127.0.0.1:1/captures",
            )
            .output()
            .unwrap(),
        "MIGRATION_DATABASE_URL is required",
    );
}

#[test]
fn startup_requires_the_migration_secret_and_rejects_the_pooler() {
    let runtime = "postgres://app:private-password@127.0.0.1:1/captures";
    failure(
        api().env("DATABASE_URL", runtime).output().unwrap(),
        "MIGRATION_DATABASE_URL is required",
    );
    failure(
        api()
            .env("DATABASE_URL", runtime)
            .env(
                "MIGRATION_DATABASE_URL",
                "postgres://owner:private-password@127.0.0.1:6432/captures",
            )
            .output()
            .unwrap(),
        "migrations require the direct PostgreSQL endpoint",
    );
}

#[test]
fn startup_does_not_serve_after_migration_connection_failure() {
    let unavailable = "postgres://app:private-password@127.0.0.1:1/captures";
    failure(
        api()
            .env("DATABASE_URL", unavailable)
            .env("MIGRATION_DATABASE_URL", unavailable)
            .output()
            .unwrap(),
        "database migration connection failed",
    );
}
