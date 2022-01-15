use std::collections::{hash_map::Entry, HashMap};
use std::io::{self, ErrorKind};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::exit;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use async_trait::async_trait;
use log::{error, info};
use structopt::StructOpt;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};
use tokio::time::{sleep, Duration};

use udp_hole_punching::file_transfer::{receive, send};
use udp_hole_punching::util::{init_logger, resolve, runtime};
use udp_hole_punching::Message::*;
use udp_hole_punching::{err, perform, Message, Operation, Result, Socket, WithContext};

#[derive(StructOpt)]
struct Opt {
    /// 外网服务器地址
    #[structopt(short, long)]
    addr: String,

    /// 外网服务器地址
    #[structopt(long)]
    addr2: String,

    /// 要发送的文件，指定本项表示这是一个发送端
    #[structopt(short, long, conflicts_with("receive"), required_unless("receive"))]
    send: Option<PathBuf>,

    /// 接收文件保存位置，指定本项表示这是一个接收端
    #[structopt(short, long, required_unless("send"))]
    receive: Option<PathBuf>,

    /// 如果作为发送端，表示接收端的 id，否则表示自己的 id
    #[structopt(long)]
    id: String,
}

const RECV_BUF_SIZE: usize = 256;

const PUNCH_HOLE_DURATION: Duration = Duration::from_secs(1);

fn main() {
    let opt: Opt = Opt::from_args();
    init_logger();

    runtime(opt.receive.is_some()).block_on(async move {
        if let Err(e) = run(opt).await {
            error!("{}", e);
            exit(1);
        }
    });
}

async fn run(opt: Opt) -> Result<()> {
    let server_addr = resolve(&opt.addr).await?;
    let server_addr2 = resolve(&opt.addr2).await?;
    let mut sock = Socket::new_unspecified().await?;
    let mut buf = vec![0u8; RECV_BUF_SIZE];

    // 如果是对称型 nat，终止
    let mut op = DetectSymmetricNat::new(&sock, server_addr, server_addr2, &mut buf);
    perform(&mut op)
        .await
        .map_err(err!("detect symmetric nat"))?;

    let id = opt.id.into_bytes();
    if opt.receive.is_some() {
        // 向服务器注册，等待连接
        sock.connect(server_addr).await.map_err(err!())?;
        let mut op = Register::new(&sock, &id, &mut buf);
        perform(&mut op).await.map_err(err!("register"))?;

        let peers = Arc::new(Mutex::new(HashMap::new()));
        loop {
            tokio::select! {
                recv = sock.recv(&mut buf) => {
                    match recv.map_err(err!())? {
                        Request { peer_addr } => {
                            match peers.lock().unwrap().entry(peer_addr) {
                                Entry::Vacant(v) => {
                                    let (tx, rx) = unbounded_channel::<()>();
                                    v.insert(tx);
                                    let peers = Arc::clone(&peers);
                                    let dir = opt.receive.clone().unwrap();
                                    tokio::spawn(async move {
                                        if let Err(e) = handle_punch(server_addr, peer_addr, rx, dir).await {
                                            error!("{}", e);
                                        }
                                        peers.lock().unwrap().remove(&peer_addr);
                                    });
                                }
                                // 防止重复处理
                                Entry::Occupied(v) => v.get().send(()).map_err(err!())?,
                            }
                        }
                        _ => {}
                    }
                }
                _ = sleep(Duration::from_secs(30)) => {
                    // 定时向服务器注册
                    let id = id.clone();
                    sock.send_to(&Message::Register { id }, server_addr).await.map_err(err!())?;
                }
            }
        }
    } else {
        // 查询 peer，发起打洞
        let mut op = Lookup::new(&sock, server_addr, &id, &mut buf);
        match perform(&mut op).await.map_err(err!("lookup"))? {
            Some(peer_addr) => {
                let ttl = sock.as_ref().ttl().map_err(err!())?;
                sock.as_ref().set_ttl(6).map_err(err!())?;
                sock.send_to(&Hello, peer_addr).await.map_err(err!())?;
                sock.as_ref().set_ttl(ttl).map_err(err!())?;

                std::thread::sleep(Duration::from_millis(50));
                sock.send_to(&Hello, peer_addr).await.map_err(err!())?;
                let deadline = Instant::now() + PUNCH_HOLE_DURATION;
                loop {
                    tokio::select! {
                        recv = sock.recv_from(&mut buf) => {
                            let (msg, src) = recv.map_err(err!())?;
                            match msg {
                                Hello if src == peer_addr => {
                                    sock.connect(peer_addr).await?;
                                    sock.send(&HelloAck).await.map_err(err!())?;
                                    break;
                                }
                                _ => {}
                            }
                            sock.send(&Hello).await.map_err(err!())?;
                        }
                        _ = sleep(Duration::from_millis(100)) => {
                            if Instant::now() < deadline {
                                sock.send_to(&Hello, peer_addr).await.map_err(err!())?;
                            } else {
                                Err(io::Error::from(ErrorKind::TimedOut)).map_err(err!("punch hole with {} failed", peer_addr))?;
                            }
                        }
                    }
                }
            }
            None => Err(io::Error::new(ErrorKind::Other, "peer not found")).map_err(err!())?,
        }

        let file = opt.send.unwrap();
        send(sock, &file).await.ctx("file", file.display())
    }
}

async fn handle_punch(
    server_addr: SocketAddr,
    peer_addr: SocketAddr,
    mut rx: UnboundedReceiver<()>,
    dir: PathBuf,
) -> Result<()> {
    let mut sock = Socket::new_unspecified().await?;
    let response = Response { peer_addr };
    sock.send_to(&response, server_addr).await.map_err(err!())?;

    let mut buf = vec![0u8; RECV_BUF_SIZE];
    let deadline = Instant::now() + PUNCH_HOLE_DURATION;
    let default_ttl = sock.as_ref().ttl().map_err(err!())?;
    let mut server_ack = false;
    let mut hello = false;

    loop {
        tokio::select! {
            recv = sock.recv_from(&mut buf) => {
                let (msg, src) = recv.map_err(err!())?;
                match msg {
                    ResponseAck if src == server_addr => {
                        server_ack = true;
                        // 使用一个较小的 TTL，在本端 NAT 留下记录，不达到对端 NAT，防止被加入黑名单
                        sock.as_ref().set_ttl(6).map_err(err!())?;
                        sock.send_to(&Hello, peer_addr).await.map_err(err!())?;
                    }
                    Hello if src == peer_addr => {
                        hello = true;
                        sock.as_ref().set_ttl(default_ttl).map_err(err!())?;
                        sock.send_to(&Hello, peer_addr).await.map_err(err!())?;
                    }
                    HelloAck if src == peer_addr => {
                        sock.as_ref().set_ttl(default_ttl).map_err(err!())?;
                        break;
                    }
                    _ => {}
                }
            }
            _ = rx.recv(), if !hello => {
                server_ack = false;
                sock.as_ref().set_ttl(default_ttl).map_err(err!())?;
                sock.send_to(&response, server_addr).await.map_err(err!())?;
            }
            _ = sleep(Duration::from_millis(150)) => {
                if Instant::now() > deadline {
                    if hello {
                        break;
                    } else {
                        Err(io::Error::from(ErrorKind::TimedOut)).map_err(err!("punch hole with {} failed", peer_addr))?;
                    }
                }
                if server_ack {
                    sock.send_to(&Hello, peer_addr).await.map_err(err!())?;
                } else {
                    sock.send_to(&response, server_addr).await.map_err(err!())?;
                }
            }
        }
    }
    sock.connect(peer_addr).await?;

    receive(sock, dir).await
}

/// 检测是否是对称型 NAT
pub struct DetectSymmetricNat<'a> {
    socket: &'a Socket,
    server_addr1: SocketAddr,
    server_addr2: SocketAddr,
    addr1: Option<SocketAddr>,
    addr2: Option<SocketAddr>,
    buf: &'a mut [u8],
}

impl<'a> DetectSymmetricNat<'a> {
    pub fn new(
        socket: &'a Socket,
        server_addr1: SocketAddr,
        server_addr2: SocketAddr,
        buf: &'a mut [u8],
    ) -> Self {
        Self {
            socket,
            server_addr1,
            server_addr2,
            addr1: None,
            addr2: None,
            buf,
        }
    }
}

#[async_trait]
impl<'a> Operation<()> for DetectSymmetricNat<'a> {
    async fn poll(&mut self) -> io::Result<()> {
        if self.addr1.is_none() {
            self.socket.send_to(&Query, self.server_addr1).await?;
        }
        if self.addr2.is_none() {
            self.socket.send_to(&Query, self.server_addr2).await?;
        }
        Ok(())
    }

    async fn resolve(&mut self) -> io::Result<()> {
        loop {
            if let (Address(addr), src) = self.socket.recv_from(&mut self.buf).await? {
                if src == self.server_addr1 {
                    self.addr1 = Some(addr);
                } else if src == self.server_addr2 {
                    self.addr2 = Some(addr);
                } else {
                    continue;
                }

                if self.addr1.is_some() && self.addr2.is_some() {
                    info!("address: {} {}", self.addr1.unwrap(), self.addr2.unwrap());
                    return if self.addr1 == self.addr2 {
                        Ok(())
                    } else {
                        Err(io::Error::new(ErrorKind::Other, "symmetric nat"))
                    };
                }
            }
        }
    }
}

/// peer 注册
pub struct Register<'a> {
    socket: &'a Socket,
    msg: Message,
    buf: &'a mut [u8],
}

impl<'a> Register<'a> {
    pub fn new(socket: &'a Socket, id: &Vec<u8>, buf: &'a mut [u8]) -> Self {
        let msg = Message::Register { id: id.clone() };
        Self { socket, msg, buf }
    }
}

#[async_trait]
impl<'a> Operation<()> for Register<'a> {
    async fn poll(&mut self) -> io::Result<()> {
        self.socket.send(&self.msg).await
    }

    async fn resolve(&mut self) -> io::Result<()> {
        loop {
            if let RegisterAck = self.socket.recv(&mut self.buf).await? {
                info!("register ok");
                return Ok(());
            }
        }
    }
}

/// 查询 peer 外网地址
pub struct Lookup<'a> {
    socket: &'a Socket,
    server_addr: SocketAddr,
    msg: Message,
    buf: &'a mut [u8],
}

impl<'a> Lookup<'a> {
    pub fn new(
        socket: &'a Socket,
        server_addr: SocketAddr,
        peer_id: &Vec<u8>,
        buf: &'a mut [u8],
    ) -> Self {
        let peer_id = peer_id.clone();
        let msg = Message::Lookup { peer_id };
        Self {
            socket,
            server_addr,
            msg,
            buf,
        }
    }
}

#[async_trait]
impl<'a> Operation<Option<SocketAddr>> for Lookup<'a> {
    async fn poll(&mut self) -> io::Result<()> {
        self.socket.send_to(&self.msg, self.server_addr).await
    }

    async fn resolve(&mut self) -> io::Result<Option<SocketAddr>> {
        loop {
            match self.socket.recv_from(&mut self.buf).await? {
                (Peer { addr }, src) if src == self.server_addr => {
                    return Ok(addr);
                }
                _ => {}
            }
        }
    }
}
