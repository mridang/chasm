# Base image is pinned by digest. The human-readable `1.95-alpine` tag is kept
# for reviewers; the `@sha256:` suffix is what Docker actually resolves.
FROM rust:1.95-alpine@sha256:606fd313a0f49743ee2a7bd49a0914bab7deedb12791f3a846a34a4711db7ed2 AS builder
RUN apk add --no-cache musl-dev upx
WORKDIR /app
COPY . .
RUN cargo build --release --bin chasm-server --locked \
 && cp target/release/chasm-server /chasm-server \
 && strip /chasm-server \
 && (upx --lzma --best /chasm-server \
       || echo "WARNING: upx compression failed, shipping uncompressed binary")

FROM scratch
# OCI image-spec labels so registries (and downstream scanners) can link the
# image back to its source, license, and project metadata. Keep these in sync
# with `[workspace.package]` in the root Cargo.toml.
LABEL org.opencontainers.image.source="https://github.com/mridang/chasm"
LABEL org.opencontainers.image.url="https://github.com/mridang/chasm"
LABEL org.opencontainers.image.licenses="MIT"
LABEL org.opencontainers.image.title="chasm-server"
LABEL org.opencontainers.image.description="OAS3 mock server"
COPY --from=builder /chasm-server /chasm-server
# Drop root. The scratch image has no /etc/passwd, so we reference the UID:GID
# numerically. 65532 is the conventional non-root UID used by distroless.
USER 65532:65532
EXPOSE 4010
ENTRYPOINT ["/chasm-server"]
