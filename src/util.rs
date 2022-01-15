use std::env::{set_var, var};
use std::io;
use std::io::ErrorKind;
use std::net::SocketAddr;

use tokio::net::lookup_host;
use tokio::runtime::{Builder, Runtime};

use crate::error::Result;

/// 创建 tokio Runtime
pub fn runtime(multi_thread: bool) -> Runtime {
    let mut builder = if multi_thread {
        Builder::new_multi_thread()
    } else {
        Builder::new_current_thread()
    };
    builder.enable_all().build().unwrap()
}

/// 初始化日志
pub fn init_logger() {
    if var("RUST_LOG").is_err() {
        #[cfg(debug_assertions)]
        set_var("RUST_LOG", "debug");
        #[cfg(not(debug_assertions))]
        set_var("RUST_LOG", "info");
    }
    env_logger::init();
}

/// 解析域名
pub async fn resolve(host: &str) -> Result<SocketAddr> {
    match lookup_host(host)
        .await
        .map_err(err!("cannot resolve {}", host))?
        .next()
    {
        Some(addr) => Ok(addr),
        None => Err(io::Error::new(
            ErrorKind::Other,
            format!("cannot resolve {}", host),
        ))
        .map_err(err!()),
    }
}
