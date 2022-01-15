//! 外网服务器，协调打洞

use std::collections::HashMap;
use std::net::SocketAddr;
use std::process::exit;
use std::time::{Duration, Instant};

use log::{error, info};
use structopt::StructOpt;

use udp_hole_punching::util::{init_logger, runtime};
use udp_hole_punching::Message::*;
use udp_hole_punching::{cont, Result, Socket};

#[derive(StructOpt)]
struct Opt {
    /// 绑定地址，格式：ip:端口
    #[structopt(long)]
    addr: SocketAddr,

    /// 绑定地址，格式：ip:端口
    #[structopt(long)]
    addr2: SocketAddr,
}

const RECV_BUF_SIZE: usize = 256;

fn main() {
    let opt: Opt = Opt::from_args();
    init_logger();
    runtime(false).block_on(async {
        if let Err(e) = run(opt).await {
            error!("{}", e);
            exit(1);
        }
    })
}

async fn run(opt: Opt) -> Result<()> {
    let sock = Socket::new(opt.addr).await?;
    let sock2 = Socket::new(opt.addr2).await?;
    info!("bind to {} {}", opt.addr, opt.addr2);

    let mut buf = [0u8; RECV_BUF_SIZE];
    let mut buf2 = [0u8; RECV_BUF_SIZE];
    let mut peers = HashMap::new();
    let mut peer_gc_at = Instant::now();

    loop {
        tokio::select! {
            recv = sock.recv_from(&mut buf) => {
                let (msg, src) = cont!(recv);
                match msg {
                    // peer 查询外网地址
                    Query => {
                        cont!(sock.send_to(&Address(src), src).await);
                    }
                    // peer 注册
                    Register { id } => {
                        peers.insert(id, (src, Instant::now()));
                        cont!(sock.send_to(&RegisterAck, src).await);
                    }
                    // peer 查询另一个 peer 的外网地址
                    Lookup { peer_id } => match peers.get(&peer_id) {
                        Some((addr, _)) => {
                            cont!(sock.send_to(&Request { peer_addr: src }, *addr).await);
                        }
                        None => cont!(sock.send_to(&Peer { addr: None }, src).await),
                    }
                    // peer 响应查询
                    Response { peer_addr } => {
                        cont!(sock.send_to(&ResponseAck, src).await);
                        cont!(sock.send_to(&Peer { addr: Some(src) }, peer_addr).await);
                    }
                    _ => {}
                }
            }
            recv = sock2.recv_from(&mut buf2) => {
                let (msg, src) = cont!(recv);
                match msg {
                    // peer 查询外网地址
                    Query => {
                        cont!(sock2.send_to(&Address(src), src).await);

                        // 清除不活跃的　peer
                        if peers.len() > 256 && peer_gc_at.elapsed() > Duration::from_secs(600) {
                            peer_gc_at = peer_gc(&mut peers);
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}

// 清除不活跃的　peer
fn peer_gc(peers: &mut HashMap<Vec<u8>, (SocketAddr, Instant)>) -> Instant {
    let now = Instant::now();
    let mut gc = Vec::new();
    for (k, v) in &*peers {
        if now.duration_since(v.1) > Duration::from_secs(600) {
            gc.push(k.clone());
        }
    }
    for v in &gc {
        peers.remove(v);
    }
    now
}
