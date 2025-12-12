# quarkdrive-webdav
夸克网盘 WebDAV 服务

[![Docker Image](https://img.shields.io/badge/version-latest-blue)](https://ghcr.io/chenqimiao/quarkdrive-webdav)
[![Crates.io](https://img.shields.io/crates/v/quarkdrive-webdav.svg)](https://crates.io/crates/quarkdrive-webdav)

 
 ## 核心特性
- 🐳 Docker 容器化部署 - 快速启动，零环境依赖，无需繁琐配置即可完成部署
- 📦 二进制包 + 命令行启动 - 支持直接下载二进制包，通过命令行快速启动，部署方式灵活多样
- 🌱 极致轻量级运行 - 仅需约 10MB 内存占用，低资源消耗特性使其可流畅运行在低配置环境中
- 🔄 NAS 与云盘双向同步 - 支持 NAS 与云盘间文件基于Webdav协议的备份、下载、上传操作，满足数据跨端管理需求
- 🎬 云盘影音无缝播放 - 完美适配  [Infuse](https://firecore.com/infuse)、[nPlayer](https://nplayer.com) 等支持 WebDAV 协议的客户端 App直接播放网盘视频



如果项目对你有帮助，欢迎 Star 或者赞助我，以支持本项目的继续开发

## 支付码

<p align="center">
  <img src="https://github.com/chenqimiao/chenqimiao/raw/main/pic/alipay.JPG" alt="alipay" width="400" height="400" style="margin-right: 40px;"/>
  <img src="https://github.com/chenqimiao/chenqimiao/raw/main/pic/wechat_pay.JPG" alt="wechat_pay" width="400" height="400"/>
</p>


> **Note**
>
> 本项目作者没有上传需求, 所以上传实现较为简单，测试场景不能全部覆盖，后续会慢慢优化

## 安装

可以从 [GitHub Releases](https://github.com/chenqimiao/quarkdrive-webdav/releases) 页面下载预先构建的二进制包


## 命令行启动

```bash
quarkdrive-webdav --quark-cookie '你的cookie' -U '用户名' -W '密码' -p 8080
```


## Docker 运行

### docker run
```bash
docker run -d --name=quarkdrive-webdav --restart=unless-stopped -p 8080:8080 \
  -e QUARK_COOKIE='your quark cookie' \
  -e WEBDAV_AUTH_USER=admin \
  -e WEBDAV_AUTH_PASSWORD=admin \
  ghcr.io/chenqimiao/quarkdrive-webdav:latest
```

### docker compose

```yaml
version: '3.8'
services:
  quarkdrive-webdav:
    image: ghcr.io/chenqimiao/quarkdrive-webdav:latest
    container_name: quarkdrive-webdav
    restart: unless-stopped
    ports:
      - "8080:8080"
    environment:
      - QUARK_COOKIE=your quark cookie
      - WEBDAV_AUTH_USER=admin
      - WEBDAV_AUTH_PASSWORD=admin
```

其中，`QUARK_COOKIE` 环境变量为你的夸克云盘 `cookie`，`WEBDAV_AUTH_USER`
和 `WEBDAV_AUTH_PASSWORD` 为连接 WebDAV 服务的用户名和密码。



启动后，用webdav客户端连接http://nas地址:8080 即可


## 🚨 免责声明

本项目仅供学习和研究目的，不得用于任何商业活动。用户在使用本项目时应遵守所在地区的法律法规，对于违法使用所导致的后果，本项目及作者不承担任何责任。
本项目可能存在未知的缺陷和风险（包括但不限于设备损坏和账号封禁等），使用者应自行承担使用本项目所产生的所有风险及责任。
作者不保证本项目的准确性、完整性、及时性、可靠性，也不承担任何因使用本项目而产生的任何损失或损害责任。
使用本项目即表示您已阅读并同意本免责声明的全部内容。



## 本项目参考了以下开源项目，特此鸣谢
- [aliyundrive-webdav](https://github.com/messense/aliyundrive-webdav)
