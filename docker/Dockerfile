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

RUN apk add --no-cache libssl3 cronie findutils \
    && rm -rf /var/cache/apk/*
RUN addgroup -S app && adduser -S app -G app -s /bin/sh
WORKDIR /app

COPY --from=builder /app/target/release/quarkdrive-webdav /usr/local/bin/quarkdrive-webdav

# 添加定时清理任务
RUN echo "0 3 * * * root find /tmp -type f -amin +15 -delete" > /etc/crontabs/root

# 用 root 用户启动 cron 和主程序
USER root
CMD crond && su -s /bin/sh app -c "/usr/local/bin/quarkdrive-webdav"
