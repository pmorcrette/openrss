# syntax=docker/dockerfile:1

# ---- build: a static (musl) binary with the web UI embedded ----
FROM rust:1-alpine AS build
WORKDIR /src

# Build dependencies first so they are cached across source changes.
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo 'fn main() {}' > src/main.rs && touch src/lib.rs \
    && cargo build --release --locked \
    && rm -rf src

COPY src src
COPY public public
RUN touch src/main.rs src/lib.rs && cargo build --release --locked \
    && mkdir /data

# ---- runtime: just the binary and CA certificates ----
FROM scratch
COPY --from=build /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt
COPY --from=build --chown=10001:10001 /data /data
COPY --from=build /src/target/release/openrss /openrss

USER 10001:10001
ENV DATA_DIR=/data \
    PORT=8080 \
    SSL_CERT_FILE=/etc/ssl/certs/ca-certificates.crt
VOLUME ["/data"]
EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s CMD ["/openrss", "healthcheck"]
ENTRYPOINT ["/openrss"]
CMD ["serve"]
