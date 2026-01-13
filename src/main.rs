use std::env;
use std::path::PathBuf;
use std::sync::{Arc};
use std::time::Duration;
use anyhow::bail;
use clap::{Parser, Subcommand};
use dashmap::DashMap;
use dav_server::{memls::MemLs, DavHandler};
#[cfg(unix)]
use futures_util::stream::StreamExt;
use tracing::{debug, info};
use tracing_subscriber::EnvFilter;
#[cfg(unix)]
use {signal_hook::consts::signal::*, signal_hook_tokio::Signals};

use cache::Cache;
use drive::*;
use vfs::QuarkDriveFileSystem;
use webdav::WebDavServer;

mod cache;
mod drive;
mod vfs;
mod webdav;
use tokio::time::interval;

#[derive(Parser, Debug)]
#[command(name = "quarkdrive-webdav", about, version, author)]
#[command(args_conflicts_with_subcommands = true)]
struct Opt {
    /// Listen host
    #[arg(long, env = "HOST", default_value = "0.0.0.0")]
    host: String,
    /// Listen port
    #[arg(short, env = "PORT", default_value = "8080")]
    port: u16,

    ///  drive client_secret
    #[arg(long, env = "QUARK_COOKIE")]
    quark_cookie: Option<String>,

    /// WebDAV authentication username
    #[arg(short = 'U', long, env = "WEBDAV_AUTH_USER")]
    auth_user: Option<String>,
    /// WebDAV authentication password
    #[arg(short = 'W', long, env = "WEBDAV_AUTH_PASSWORD")]
    auth_password: Option<String>,
    /// Automatically generate index.html
    #[arg(short = 'I', long)]
    auto_index: bool,
    /// Read/download buffer size in bytes, defaults to 10MB
    #[arg(short = 'S', long, default_value = "10485760")]
    read_buffer_size: usize,
    /// Upload buffer size in bytes, defaults to 16MB
    #[arg(long, default_value = "16777216")]
    upload_buffer_size: usize,
    /// Directory entries cache size
    #[arg(long, default_value = "1000")]
    cache_size: u64,
    /// Directory entries cache expiration time in seconds
    #[arg(long, default_value = "600")]
    cache_ttl: u64,
    /// Root directory path
    #[arg(long, env = "WEBDAV_ROOT", default_value = "/")]
    root: String,
    /// Delete file permanently instead of trashing it
    #[arg(long)]
    no_trash: bool,
    /// Enable read only mode
    #[arg(long)]
    read_only: bool,
    /// TLS certificate file path
    #[arg(long, env = "TLS_CERT")]
    tls_cert: Option<PathBuf>,
    /// TLS private key file path
    #[arg(long, env = "TLS_KEY")]
    tls_key: Option<PathBuf>,
    /// Prefix to be stripped off when handling request.
    #[arg(long, env = "WEBDAV_STRIP_PREFIX")]
    strip_prefix: Option<String>,
    /// Enable debug log
    #[arg(long)]
    debug: bool,
    /// Disable self auto upgrade
    #[arg(long)]
    no_self_upgrade: bool,
    /// Skip uploading same size file
    #[arg(long)]
    skip_upload_same_size: bool,
    /// Prefer downloading using HTTP protocol
    #[arg(long)]
    prefer_http_download: bool,
    /// Enable 302 redirect when possible
    #[arg(long)]
    redirect: bool,

    #[command(subcommand)]
    subcommands: Option<Commands>,

    #[arg(long, env = "REFRESH_CACHE_SECS_INTERVAL", default_value = "300")]
    refresh_cache_secs_interval: u64,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Scan QRCode
    #[command(subcommand)]
    Qr(QrCommand),
}

#[derive(Subcommand, Debug)]
enum QrCommand {
    /// Scan QRCode login to get a token
    Login,
    /// Generate a QRCode
    Generate,
    /// Query the QRCode login result
    #[command(arg_required_else_help = true)]
    Query {
        /// Query parameter sid
        #[arg(long)]
        sid: String,
    },
}

pub fn start_periodic_invalidate(cache: Arc<Cache>, secs: u64) {
    tokio::spawn(async move {
        let mut ticker = interval(Duration::from_secs(secs));
        loop {
            ticker.tick().await;
            cache.invalidate_all();
        }
    });
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> anyhow::Result<()> {
    #[cfg(feature = "native-tls-vendored")]
    openssl_probe::init_openssl_env_vars();
    let opt = Opt::parse();
    if env::var("RUST_LOG").is_err() {
        if opt.debug {
            unsafe { env::set_var("RUST_LOG", "quarkdrive_webdav=debug,reqwest=debug"); }
        } else {
            unsafe { env::set_var("RUST_LOG", "quarkdrive_webdav=info,reqwest=warn"); }
        }
    }
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_timer(tracing_subscriber::fmt::time::time())
        .init();

    let cookie_str = opt.quark_cookie.unwrap_or_else(||{ 
        panic!("QUARK_COOKIE must be specified. Please set it in the environment or use --quark-cookie option.");
    });
    let init_cookie = Arc::new(DashMap::new());
    for pair in cookie_str.split(';') {
        if let Some((k, v)) = pair.trim().split_once('=') {
            init_cookie.insert(k.trim().to_string(), v.trim().to_string());
        }
    }
    let drive_config = DriveConfig {
        api_base_url: "https://drive.quark.cn".to_string(),
        cookie: init_cookie,
    };
    let auth_user = opt.auth_user;
    let auth_password = opt.auth_password;
    if (auth_user.is_some() && auth_password.is_none())
        || (auth_user.is_none() && auth_password.is_some())
    {
        bail!("auth-user and auth-password must be specified together.");
    }

    let tls_config = match (opt.tls_cert, opt.tls_key) {
        (Some(cert), Some(key)) => Some((cert, key)),
        (None, None) => None,
        _ => bail!("tls-cert and tls-key must be specified together."),
    };
    let drive = QuarkDrive::new(drive_config)?;
    let mut fs = QuarkDriveFileSystem::new(drive, opt.root, opt.cache_size, opt.cache_ttl)?;
    fs.set_no_trash(opt.no_trash)
        .set_read_only(opt.read_only)
        .set_upload_buffer_size(opt.upload_buffer_size)
        .set_skip_upload_same_size(opt.skip_upload_same_size)
        .set_prefer_http_download(opt.prefer_http_download);
    let cache = Arc::new(fs.dir_cache.clone());
    start_periodic_invalidate(cache.clone(), opt.refresh_cache_secs_interval);
    let mut dav_server_builder = DavHandler::builder()
        .filesystem(Box::new(fs))
        .locksystem(MemLs::new())
        .read_buf_size(opt.read_buffer_size)
        .autoindex(opt.auto_index)
        .redirect(opt.redirect);
    if let Some(prefix) = opt.strip_prefix {
        dav_server_builder = dav_server_builder.strip_prefix(prefix);
    }

    let dav_server = dav_server_builder.build_handler();
    debug!(
        read_buffer_size = opt.read_buffer_size,
        auto_index = opt.auto_index,
        "webdav handler initialized"
    );

    let server = WebDavServer {
        host: opt.host,
        port: opt.port,
        auth_user,
        auth_password,
        tls_config,
        handler: dav_server,
    };

    #[cfg(not(unix))]
    server.serve().await?;
    #[cfg(unix)]
    {
        let signals = Signals::new([SIGHUP])?;
        let handle = signals.handle();
        let signals_task = tokio::spawn(handle_signals(signals, cache));

        server.serve().await?;

        // Terminate the signal stream.
        handle.close();
        signals_task.await?;
    }
    Ok(())
}

#[cfg(unix)]
async fn handle_signals(mut signals: Signals, dir_cache: Arc<Cache>) {
    while let Some(signal) = signals.next().await {
        match signal {
            SIGHUP => {
                dir_cache.invalidate_all();
                info!("directory cache invalidated by SIGHUP");
            }
            _ => unreachable!(),
        }
    }
}
