# 构建阶段：Alpine + Rust nightly
FROM alpine:3.19 AS builder

RUN apk add --no-cache build-base musl-dev openssl-dev openssl-libs-static pkgconf curl binutils perl \
    && curl https://sh.rustup.rs -sSf | sh -s -- -y --default-toolchain nightly

ENV PATH="/root/.cargo/bin:${PATH}"

WORKDIR /app
COPY . .
RUN cargo fetch
RUN cargo build --release && strip target/release/quarkdrive-webdav

# 运行阶段：极简，仅需运行依赖
FROM alpine:3.19

RUN apk add --no-cache libssl3
RUN addgroup -S app && adduser -S app -G app
WORKDIR /app

COPY --from=builder /app/target/release/quarkdrive-webdav /usr/local/bin/quarkdrive-webdav

USER app
ENTRYPOINT ["/usr/local/bin/quarkdrive-webdav"]