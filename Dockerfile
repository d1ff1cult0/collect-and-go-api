# syntax=docker/dockerfile:1

# ---- build ---------------------------------------------------------------------------------
FROM rust:1-slim-trixie AS build
WORKDIR /app

# Eerst enkel de dependencies bouwen, zodat die laag gecachet blijft bij codewijzigingen.
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo 'fn main() {}' > src/main.rs \
    && cargo build --release --locked \
    && rm -rf src target/release/deps/cogo_prices* target/release/cogo-prices* \
              target/release/.fingerprint/cogo-prices-*

COPY migrations ./migrations
COPY templates ./templates
COPY static ./static
COPY src ./src
# `touch`: COPY behoudt oude mtimes, waardoor cargo anders de dummy-build als actueel zou zien.
RUN touch src/main.rs && cargo build --release --locked && strip target/release/cogo-prices \
    && ./target/release/cogo-prices --help 2>&1 | grep -q healthcheck

# ---- runtime -------------------------------------------------------------------------------
FROM debian:trixie-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates tzdata \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --home /data cogo \
    && mkdir -p /data && chown cogo /data

COPY --from=build /app/target/release/cogo-prices /usr/local/bin/cogo-prices

USER cogo
WORKDIR /data
ENV BIND_ADDR=0.0.0.0:8080 \
    DATABASE_URL=sqlite:///data/cogo.db \
    SCRAPE_INTERVAL_HOURS=6 \
    RUST_LOG=info,sqlx=warn
VOLUME ["/data"]
EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=6s --start-period=20s --retries=3 CMD ["cogo-prices", "healthcheck"]
ENTRYPOINT ["cogo-prices"]
CMD ["serve"]
