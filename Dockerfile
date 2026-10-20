# syntax=docker/dockerfile:1
FROM rust:1.97-slim-trixie AS build
WORKDIR /src

# Build dependencies first so code changes don't rebuild them.
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo 'fn main() {}' > src/main.rs \
    && cargo build --release --locked \
    && rm -rf src

COPY migrations migrations
COPY src src
RUN touch src/main.rs && cargo build --release --locked

FROM debian:trixie-slim
# CA certificates: AiNKI fetches OAuth client metadata over HTTPS.
COPY --from=build /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt
RUN useradd --system --uid 10001 --home-dir /data --shell /usr/sbin/nologin ainki \
    && mkdir -p /data && chown ainki:ainki /data
COPY --from=build /src/target/release/ainki /usr/local/bin/ainki
USER ainki
ENV AINKI_DATA_DIR=/data \
    AINKI_BIND=0.0.0.0:8080
VOLUME /data
EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s CMD ["ainki", "healthcheck"]
CMD ["ainki"]
