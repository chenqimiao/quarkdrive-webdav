use std::future::Future;
use std::io;
use std::net::ToSocketAddrs;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use anyhow::Result;
use dav_server::{body::Body, DavConfig, DavHandler};
use headers::{authorization::Basic, Authorization, HeaderMapExt};
use hyper::service::Service;
use hyper::{Method, Request, Response};
use hyper_util::{
    rt::{TokioExecutor, TokioIo},
    server::conn::auto,
};
use tokio::net::TcpListener;
use tracing::{debug, error, info};

use crate::vfs::QuarkDriveFileSystem;

pub struct WebDavServer {
    pub host: String,
    pub port: u16,
    pub auth_user: Option<String>,
    pub auth_password: Option<String>,
    pub tls_config: Option<(PathBuf, PathBuf)>,
    pub handler: DavHandler,
    pub fs: QuarkDriveFileSystem,
    pub strip_prefix: Option<String>,
}

impl WebDavServer {
    pub async fn serve(self) -> Result<()> {
        let addr = (self.host, self.port)
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| io::Error::from(io::ErrorKind::AddrNotAvailable))?;

        let make_svc = MakeSvc {
            auth_user: self.auth_user.clone(),
            auth_password: self.auth_password.clone(),
            handler: self.handler.clone(),
            fs: self.fs.clone(),
            strip_prefix: self.strip_prefix.clone(),
        };

        let listener = TcpListener::bind(&addr).await?;
        info!("listening on http://{}", listener.local_addr()?);

        loop {
            let (tcp, _) = listener.accept().await?;
            let io = TokioIo::new(tcp);
            let make_svc = make_svc.clone();

            tokio::spawn(async move {
                let service = match make_svc.call(()).await {
                    Ok(service) => service,
                    Err(_) => return,
                };

                if let Err(e) = auto::Builder::new(TokioExecutor::new())
                    .serve_connection(io, service)
                    .await
                {
                    error!("HTTP serve error: {}", e);
                }
            });
        }

        // 循环会持续运行，实际不会到达这里
        Ok(())
    }
}

#[derive(Clone)]
pub struct QuarkDriveWebDav {
    auth_user: Option<String>,
    auth_password: Option<String>,
    handler: DavHandler,
    fs: QuarkDriveFileSystem,
    strip_prefix: Option<String>,
}

impl QuarkDriveWebDav {
    fn is_browser_request(req: &Request<hyper::body::Incoming>) -> bool {
        if req.method() != Method::GET {
            return false;
        }
        if let Some(accept) = req.headers().get("accept") {
            if let Ok(accept_str) = accept.to_str() {
                return accept_str.contains("text/html");
            }
        }
        false
    }

    fn compute_fs_path(&self, req_path: &str) -> PathBuf {
        let path = if let Some(ref prefix) = self.strip_prefix {
            let prefix = prefix.trim_end_matches('/');
            req_path
                .strip_prefix(prefix)
                .unwrap_or(req_path)
        } else {
            req_path
        };

        let path = path.trim_start_matches('/');
        let path = path.trim_end_matches('/');
        let path = percent_decode(path);

        if self.fs.root == Path::new("/") {
            if path.is_empty() {
                PathBuf::from("/")
            } else {
                PathBuf::from("/").join(&path)
            }
        } else if path.is_empty() {
            self.fs.root.clone()
        } else {
            self.fs.root.join(&path)
        }
    }

    async fn handle_browser_request(
        &self,
        req_path: &str,
    ) -> Option<Response<Body>> {
        let fs_path = self.compute_fs_path(req_path);
        debug!(req_path = %req_path, fs_path = %fs_path.display(), "browser: checking path");

        let files = self.fs.dir_cache.get_or_insert(&fs_path.to_string_lossy()).await?;
        debug!(req_path = %req_path, count = files.len(), "browser: directory listing");

        let html = render_directory_html(req_path, &files);
        Some(
            Response::builder()
                .status(200)
                .header("Content-Type", "text/html; charset=utf-8")
                .body(Body::from(html))
                .unwrap(),
        )
    }
}

fn percent_decode(s: &str) -> String {
    percent_encoding::percent_decode_str(s)
        .decode_utf8_lossy()
        .into_owned()
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#x27;")
}

fn format_size(size: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    const GB: u64 = 1024 * MB;
    const TB: u64 = 1024 * GB;

    if size >= TB {
        format!("{:.1} TB", size as f64 / TB as f64)
    } else if size >= GB {
        format!("{:.1} GB", size as f64 / GB as f64)
    } else if size >= MB {
        format!("{:.1} MB", size as f64 / MB as f64)
    } else if size >= KB {
        format!("{:.1} KB", size as f64 / KB as f64)
    } else {
        format!("{} B", size)
    }
}

fn format_timestamp(ts_millis: u64) -> String {
    let secs = (ts_millis / 1000) as i64;
    let dt = chrono::DateTime::from_timestamp(secs, 0);
    match dt {
        Some(dt) => {
            use chrono::FixedOffset;
            let china = FixedOffset::east_opt(8 * 3600).unwrap();
            dt.with_timezone(&china).format("%Y-%m-%d %H:%M").to_string()
        }
        None => "-".to_string(),
    }
}

fn render_directory_html(req_path: &str, files: &[crate::drive::QuarkFile]) -> String {
    let display_path = percent_decode(req_path);
    let display_path = if display_path.is_empty() || display_path == "/" {
        "/".to_string()
    } else {
        display_path
    };

    let req_path_normalized = if req_path.ends_with('/') || req_path.is_empty() {
        req_path.to_string()
    } else {
        format!("{}/", req_path)
    };

    // Build breadcrumbs
    let mut breadcrumbs = String::from(r#"<a href="/">根目录</a>"#);
    if display_path != "/" {
        let parts: Vec<&str> = display_path.trim_matches('/').split('/').collect();
        let mut href = String::new();
        for (i, part) in parts.iter().enumerate() {
            href.push('/');
            href.push_str(&percent_encode_path(part));
            if i == parts.len() - 1 {
                breadcrumbs.push_str(&format!(
                    r#" / <span class="current">{}</span>"#,
                    html_escape(part)
                ));
            } else {
                breadcrumbs.push_str(&format!(
                    r#" / <a href="{}">{}</a>"#,
                    html_escape(&format!("{}/", href)),
                    html_escape(part)
                ));
            }
        }
    }

    // Separate dirs and files, sort by name
    let mut dirs: Vec<&crate::drive::QuarkFile> = files.iter().filter(|f| f.dir).collect();
    let mut regular_files: Vec<&crate::drive::QuarkFile> = files.iter().filter(|f| f.file).collect();
    dirs.sort_by(|a, b| a.file_name.to_lowercase().cmp(&b.file_name.to_lowercase()));
    regular_files.sort_by(|a, b| a.file_name.to_lowercase().cmp(&b.file_name.to_lowercase()));

    let mut rows = String::new();

    // Parent directory link
    if display_path != "/" {
        rows.push_str(r#"<tr class="parent"><td class="icon">📁</td><td class="name"><a href="../">..</a></td><td class="size">-</td><td class="date">-</td></tr>"#);
    }

    for dir in &dirs {
        let name = html_escape(&dir.file_name);
        let href = format!("{}{}/", req_path_normalized, percent_encode_path(&dir.file_name));
        let date = format_timestamp(dir.updated_at);
        rows.push_str(&format!(
            r#"<tr class="dir"><td class="icon">📁</td><td class="name"><a href="{}">{}</a></td><td class="size">-</td><td class="date">{}</td></tr>"#,
            html_escape(&href), name, date
        ));
    }

    for file in &regular_files {
        let name = html_escape(&file.file_name);
        let href = format!("{}{}", req_path_normalized, percent_encode_path(&file.file_name));
        let size = format_size(file.size);
        let date = format_timestamp(file.updated_at);
        let icon = file_icon(&file.file_name);
        rows.push_str(&format!(
            r#"<tr class="file"><td class="icon">{}</td><td class="name"><a href="{}">{}</a></td><td class="size">{}</td><td class="date">{}</td></tr>"#,
            icon, html_escape(&href), name, size, date
        ));
    }

    let total = dirs.len() + regular_files.len();

    format!(
        r#"<!DOCTYPE html>
<html lang="zh-CN">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>QuarkDrive - {title}</title>
<link rel="icon" type="image/svg+xml" href="data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 64 64'%3E%3Crect width='64' height='64' rx='12' fill='%232563eb'/%3E%3Ctext x='32' y='40' font-family='Arial,Helvetica,sans-serif' font-size='26' font-weight='bold' fill='white' text-anchor='middle'%3EQW%3C/text%3E%3C/svg%3E">
<style>
* {{ margin: 0; padding: 0; box-sizing: border-box; }}
body {{ font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif; background: #f5f5f5; color: #333; line-height: 1.6; }}
.container {{ max-width: 960px; margin: 0 auto; padding: 20px; }}
.header {{ background: #fff; border-radius: 8px; padding: 16px 24px; margin-bottom: 16px; box-shadow: 0 1px 3px rgba(0,0,0,0.1); }}
.header h1 {{ font-size: 18px; font-weight: 600; color: #1a1a1a; margin-bottom: 8px; }}
.breadcrumb {{ font-size: 14px; color: #666; }}
.breadcrumb a {{ color: #2563eb; text-decoration: none; }}
.breadcrumb a:hover {{ text-decoration: underline; }}
.breadcrumb .current {{ color: #333; font-weight: 500; }}
.content {{ background: #fff; border-radius: 8px; box-shadow: 0 1px 3px rgba(0,0,0,0.1); overflow: hidden; }}
table {{ width: 100%; border-collapse: collapse; }}
thead {{ background: #fafafa; }}
th {{ text-align: left; padding: 12px 16px; font-size: 13px; font-weight: 600; color: #666; border-bottom: 1px solid #eee; }}
td {{ padding: 10px 16px; border-bottom: 1px solid #f0f0f0; font-size: 14px; }}
tr:hover {{ background: #f8fafc; }}
tr.parent:hover {{ background: #f0f7ff; }}
tr.dir:hover {{ background: #f0f7ff; }}
.icon {{ width: 32px; text-align: center; }}
.name {{ word-break: break-all; }}
.name a {{ color: #1a1a1a; text-decoration: none; }}
.name a:hover {{ color: #2563eb; text-decoration: underline; }}
.dir .name a {{ font-weight: 500; }}
.size {{ width: 100px; text-align: right; color: #888; white-space: nowrap; }}
.date {{ width: 160px; color: #888; white-space: nowrap; }}
.footer {{ text-align: center; padding: 16px; font-size: 12px; color: #aaa; }}
.footer a {{ color: #aaa; text-decoration: none; }}
.footer a:hover {{ color: #2563eb; text-decoration: underline; }}
@media (max-width: 640px) {{
  .container {{ padding: 10px; }}
  .date {{ display: none; }}
  th:last-child {{ display: none; }}
  .size {{ width: 80px; }}
}}
</style>
</head>
<body>
<div class="container">
  <div class="header">
    <h1><a href="https://github.com/chenqimiao/quarkdrive-webdav" target="_blank" style="color:inherit;text-decoration:none;">QuarkDrive WebDAV</a></h1>
    <div class="breadcrumb">{breadcrumbs}</div>
  </div>
  <div class="content">
    <table>
      <thead><tr><th class="icon"></th><th>名称</th><th class="size">大小</th><th class="date">修改时间</th></tr></thead>
      <tbody>{rows}</tbody>
    </table>
  </div>
  <div class="footer">{total} 个项目 · <a href="https://github.com/chenqimiao/quarkdrive-webdav" target="_blank">GitHub</a></div>
</div>
</body>
</html>"#,
        title = html_escape(&display_path),
        breadcrumbs = breadcrumbs,
        rows = rows,
        total = total,
    )
}

fn percent_encode_path(s: &str) -> String {
    percent_encoding::utf8_percent_encode(s, percent_encoding::NON_ALPHANUMERIC).to_string()
}

fn file_icon(name: &str) -> &'static str {
    let ext = name.rsplit('.').next().unwrap_or("").to_lowercase();
    match ext.as_str() {
        "jpg" | "jpeg" | "png" | "gif" | "bmp" | "webp" | "svg" | "ico" => "🖼️",
        "mp4" | "avi" | "mkv" | "mov" | "wmv" | "flv" | "webm" | "m4v" | "ts" => "🎬",
        "mp3" | "wav" | "flac" | "aac" | "ogg" | "wma" | "m4a" => "🎵",
        "pdf" => "📕",
        "doc" | "docx" => "📝",
        "xls" | "xlsx" => "📊",
        "ppt" | "pptx" => "📎",
        "zip" | "rar" | "7z" | "tar" | "gz" | "bz2" | "xz" => "📦",
        "txt" | "md" | "log" | "csv" => "📄",
        "exe" | "msi" | "dmg" | "app" | "deb" | "rpm" => "⚙️",
        "html" | "css" | "js" | "json" | "xml" | "yaml" | "yml" | "toml" => "💻",
        "rs" | "py" | "java" | "c" | "cpp" | "go" | "rb" | "php" | "sh" => "💻",
        _ => "📄",
    }
}

impl Service<Request<hyper::body::Incoming>> for QuarkDriveWebDav {
    type Response = Response<Body>;
    type Error = hyper::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn call(&self, req: Request<hyper::body::Incoming>) -> Self::Future {
        let should_auth = self.auth_user.is_some() && self.auth_password.is_some();
        let dav_server = self.handler.clone();
        let auth_user = self.auth_user.clone();
        let auth_pwd = self.auth_password.clone();
        let is_browser = Self::is_browser_request(&req);
        let req_path = req.uri().path().to_string();
        let browser_handler = self.clone();

        Box::pin(async move {
            if should_auth {
                let auth_user_val = auth_user.clone().unwrap();
                let auth_pwd_val = auth_pwd.clone().unwrap();

                let user = match req.headers().typed_get::<Authorization<Basic>>() {
                    Some(Authorization(basic))
                    if basic.username() == auth_user_val && basic.password() == auth_pwd_val =>
                        {
                            basic.username().to_string()
                        }
                    _ => {
                        return Ok(Response::builder()
                            .status(401)
                            .header("WWW-Authenticate", "Basic realm=\"quarkdrive-webdav\"")
                            .body(Body::from("Authentication required"))
                            .unwrap());
                    }
                };

                if is_browser {
                    if let Some(resp) = browser_handler.handle_browser_request(&req_path).await {
                        return Ok(resp);
                    }
                }

                let config = DavConfig::new().principal(user);
                Ok(dav_server.handle_with(config, req).await)
            } else {
                if is_browser {
                    if let Some(resp) = browser_handler.handle_browser_request(&req_path).await {
                        return Ok(resp);
                    }
                }

                Ok(dav_server.handle(req).await)
            }
        })
    }
}

#[derive(Clone)]
pub struct MakeSvc {
    pub auth_user: Option<String>,
    pub auth_password: Option<String>,
    pub handler: DavHandler,
    pub fs: QuarkDriveFileSystem,
    pub strip_prefix: Option<String>,
}

impl Service<()> for MakeSvc {
    type Response = QuarkDriveWebDav;
    type Error = hyper::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn call(&self, _: ()) -> Self::Future {
        let auth_user = self.auth_user.clone();
        let auth_password = self.auth_password.clone();
        let handler = self.handler.clone();
        let fs = self.fs.clone();
        let strip_prefix = self.strip_prefix.clone();

        Box::pin(async move {
            Ok(QuarkDriveWebDav {
                auth_user,
                auth_password,
                handler,
                fs,
                strip_prefix,
            })
        })
    }
}
