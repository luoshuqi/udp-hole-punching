use std::fmt::Debug;
use std::io;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};

use log::{debug, info};
use tokio::net::UdpSocket;

use crate::error::Result;

pub trait Encode {
    fn encode(&self) -> Vec<u8>;
}

pub trait Decode: Sized {
    fn decode(data: &[u8]) -> Option<Self>;
}

pub struct Socket {
    inner: UdpSocket,
    /// connect 地址，用来记录日志
    connect: Option<SocketAddr>,
}

impl Socket {
    pub async fn new(addr: SocketAddr) -> Result<Self> {
        let inner = UdpSocket::bind(addr)
            .await
            .map_err(err!("cannot bind to {}", addr))?;
        Ok(Self {
            inner,
            connect: None,
        })
    }

    pub async fn new_unspecified() -> Result<Self> {
        let addr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0));
        Self::new(addr).await
    }

    pub async fn connect(&mut self, addr: SocketAddr) -> Result<()> {
        info!("connect to {}", addr);
        self.inner
            .connect(addr)
            .await
            .map_err(err!("cannot connect to {}", addr))?;
        self.connect = Some(addr);
        Ok(())
    }

    pub fn connected_addr(&self) -> Option<SocketAddr> {
        self.connect
    }

    pub async fn send(&self, msg: &(impl Encode + Debug)) -> io::Result<()> {
        debug_assert!(self.connect.is_some());
        debug!("send {:?} to {}", msg, self.connect.unwrap());
        self.inner.send(&msg.encode()).await?;
        Ok(())
    }

    pub async fn send_to(&self, msg: &(impl Encode + Debug), addr: SocketAddr) -> io::Result<()> {
        debug!("send {:?} to {}", msg, addr);
        self.inner.send_to(&msg.encode(), addr).await?;
        Ok(())
    }

    pub async fn recv<T: Decode + Debug>(&self, buf: &mut [u8]) -> io::Result<T> {
        debug_assert!(self.connect.is_some());

        loop {
            let n = self.inner.recv(buf).await?;
            if let Some(msg) = T::decode(&buf[..n]) {
                debug!("receive {:?} from {}", msg, self.connect.unwrap());
                return Ok(msg);
            }
        }
    }

    pub async fn recv_from<T: Decode + Debug>(
        &self,
        buf: &mut [u8],
    ) -> io::Result<(T, SocketAddr)> {
        loop {
            let (n, addr) = self.inner.recv_from(buf).await?;
            if let Some(msg) = T::decode(&buf[..n]) {
                debug!("receive {:?} from {}", msg, addr);
                return Ok((msg, addr));
            }
        }
    }
}

impl AsRef<UdpSocket> for Socket {
    fn as_ref(&self) -> &UdpSocket {
        &self.inner
    }
}
