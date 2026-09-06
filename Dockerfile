# Build a statically linked agent so the runtime image can be scratch-thin and
# the same binary also works outside a container.
FROM rust:1.85-alpine AS build

RUN apk add --no-cache musl-dev

WORKDIR /src
# Copy manifests first so dependency compilation is cached across source edits.
COPY Cargo.toml Cargo.lock ./
COPY crates/scan-core/Cargo.toml crates/scan-core/
COPY crates/store/Cargo.toml crates/store/
COPY crates/diff/Cargo.toml crates/diff/
COPY crates/cli/Cargo.toml crates/cli/
COPY crates/agent/Cargo.toml crates/agent/
RUN mkdir -p crates/scan-core/src crates/store/src crates/diff/src crates/cli/src crates/agent/src \
    && echo "" > crates/scan-core/src/lib.rs \
    && echo "" > crates/store/src/lib.rs \
    && echo "" > crates/diff/src/lib.rs \
    && echo "" > crates/agent/src/lib.rs \
    && echo "fn main() {}" > crates/cli/src/main.rs \
    && echo "fn main() {}" > crates/agent/src/main.rs \
    && cargo build --release --bin spacetrace-agent 2>/dev/null || true

COPY . .
# Touch the real sources so cargo does not reuse the stub build above.
RUN find crates -name '*.rs' -exec touch {} + \
    && cargo build --release --bin spacetrace-agent --bin spacetrace

FROM alpine:3.21
RUN apk add --no-cache ca-certificates \
    && adduser -S -H -u 10001 spacetrace

COPY --from=build /src/target/release/spacetrace-agent /usr/local/bin/
COPY --from=build /src/target/release/spacetrace /usr/local/bin/

# Snapshots live here; mount a volume over it to keep history across restarts.
RUN mkdir -p /var/lib/spacetrace && chown spacetrace /var/lib/spacetrace
VOLUME /var/lib/spacetrace

USER spacetrace
EXPOSE 7878

# In a container the agent has to listen on all interfaces to be reachable at
# all, which is why this differs from the loopback default in the config file.
# Mount the host filesystem read-only at /host and scan that:
#   docker run -v /:/host:ro -v spacetrace:/var/lib/spacetrace \
#     -e SPACETRACE_TOKEN=... -p 7878:7878 spacetrace
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s \
    CMD wget -qO- http://127.0.0.1:7878/health || exit 1

ENTRYPOINT ["/usr/local/bin/spacetrace-agent"]
CMD ["--config", "/etc/spacetrace/agent.toml", "serve"]
