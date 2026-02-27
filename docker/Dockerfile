# 运行阶段：极简 Alpine 镜像
# 二进制文件由 CI 预先交叉编译，通过 TARGETARCH 自动选择对应架构
FROM alpine:3.19

ARG TARGETARCH

RUN apk add --no-cache cronie findutils && rm -rf /var/cache/apk/*
RUN addgroup -S app && adduser -S app -G app -s /bin/sh
WORKDIR /app

COPY bin/${TARGETARCH}/quarkdrive-webdav /usr/local/bin/quarkdrive-webdav
RUN chmod +x /usr/local/bin/quarkdrive-webdav

# 添加定时清理任务
RUN echo "0 3 * * * find /tmp -type f -amin +15 -mmin +15 -cmin +15 -delete" > /etc/crontabs/root

# 用 root 用户启动 cron 和主程序
USER root
CMD crond && su -s /bin/sh app -c "/usr/local/bin/quarkdrive-webdav"
