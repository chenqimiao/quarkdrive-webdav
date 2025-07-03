use std::future::Future;
use std::io;
use std::net::ToSocketAddrs;
use std::path::PathBuf;
use std::pin::Pin;
use anyhow::Result;
use dav_server::{body::Body, DavConfig, DavHandler};
use headers::{authorization::Basic, Authorization, HeaderMapExt};
use hyper::service::Service;
use hyper::{Request, Response};
use hyper_util::{
    rt::{TokioExecutor, TokioIo},
    server::conn::auto,
};
use tokio::net::TcpListener;
use tracing::{error, info};

// #[cfg(feature = "rustls-tls")]
// use {
//     std::path::Path,
//     std::sync::Arc,
// };

pub struct WebDavServer {
    pub host: String,
    pub port: u16,
    pub auth_user: Option<String>,
    pub auth_password: Option<String>,
    pub tls_config: Option<(PathBuf, PathBuf)>,
    pub handler: DavHandler,
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

        Box::pin(async move {
            if should_auth {
                let auth_user = auth_user.unwrap();
                let auth_pwd = auth_pwd.unwrap();

                let user = match req.headers().typed_get::<Authorization<Basic>>() {
                    Some(Authorization(basic))
                    if basic.username() == auth_user && basic.password() == auth_pwd =>
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

                let config = DavConfig::new().principal(user);
                Ok(dav_server.handle_with(config, req).await)
            } else {
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
}

impl Service<()> for MakeSvc {
    type Response = QuarkDriveWebDav;
    type Error = hyper::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn call(&self, _: ()) -> Self::Future {
        let auth_user = self.auth_user.clone();
        let auth_password = self.auth_password.clone();
        let handler = self.handler.clone();

        Box::pin(async move {
            Ok(QuarkDriveWebDav {
                auth_user,
                auth_password,
                handler,
            })
        })
    }
}