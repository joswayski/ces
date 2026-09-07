# Build from the repository root: docker build -f apps/api/Dockerfile -t captures-api .
FROM rust:1.94-bookworm AS build
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY apps/api apps/api
# Cargo resolves all workspace manifests even when building only the API.
COPY apps/desktop/src-tauri apps/desktop/src-tauri
COPY crates crates
RUN cargo build --locked --release -p captures-api

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 captures
COPY --from=build /app/target/release/captures-api /usr/local/bin/captures-api
USER captures
ENV CAPTURES_API_BIND=0.0.0.0:3001
EXPOSE 3001
ENTRYPOINT ["captures-api"]
