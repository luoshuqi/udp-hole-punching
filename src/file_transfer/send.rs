use std::fmt::{Display, Formatter};
use std::fs::File;
use std::io;
use std::io::ErrorKind;
use std::path::Path;

use async_trait::async_trait;
use log::info;
use tokio::time::{sleep, Duration, Instant};

use crate::file_transfer::block::{Block, BlockReader};
use crate::file_transfer::message::Chunk;
use crate::file_transfer::{Message, Request, Response};
use crate::{perform, Operation, Socket};

/// 读取超时时间
const READ_TIMEOUT: u64 = 5;

/// 发送文件
pub async fn send(sock: Socket, path: &Path) -> crate::Result<()> {
    let file = File::open(&path).map_err(err!("cannot open {}", path.display()))?;
    let file_size = path.metadata().map_err(err!())?.len();
    let name = path.file_name().unwrap().to_string_lossy().to_string();

    info!("sending {}", path.display());

    let mut buf = vec![0; 512];
    let mut op = SendRequest::new(&sock, &mut buf, name, file_size);
    let response = match perform(&mut op).await.map_err(err!("send request"))? {
        Some(v) => v,
        None => {
            info!("send {} complete", path.display());
            sock.send(&Message::FileCompleteAck).await.map_err(err!())?;
            return Ok(());
        }
    };

    let mut st = Statistic::default();
    let mut reader = BlockReader::new(
        file,
        file_size,
        response.block_size,
        response.chunk_size,
        response.start_block,
    )?;
    loop {
        match reader.read()? {
            Some(block) => send_block(&sock, &mut buf, block, &mut st).await?,
            None => break,
        }
    }

    loop {
        tokio::select! {
            msg = sock.recv(&mut buf) => {
                if let Message::FileComplete = msg.map_err(err!())? {
                    sock.send(&Message::FileCompleteAck).await.map_err(err!())?;
                    break;
                }
            }
            _ = sleep(Duration::from_secs(READ_TIMEOUT)) => {
                Err(io::Error::from(ErrorKind::TimedOut)).map_err(err!("wait complete"))?;
            }
        }
    }

    info!("send {} complete", path.display());
    info!("{}", st);
    Ok(())
}

/// 发送分块
async fn send_block(
    sock: &Socket,
    buf: &mut [u8],
    block: Block<'_>,
    st: &mut Statistic,
) -> crate::Result<()> {
    let mut chunk = 0;
    for data in block.chunks() {
        let msg = Chunk::new(block.index(), chunk, data);
        sock.send(&msg).await.map_err(err!())?;
        chunk += 1;
    }
    st.chunk += chunk as u64;

    loop {
        let mut op = SendBlockComplete::new(&sock, buf, block.index());
        let missing = perform(&mut op).await.map_err(err!())?;
        if missing.is_empty() {
            break;
        }
        st.resend_chunk += missing.len() as u64;
        for c in missing {
            let data = block
                .get_chunk(c)
                .expect(&format!("chunk {} out of range", c));
            let msg = Chunk::new(block.index(), c, data);
            sock.send(&msg).await.map_err(err!())?;
        }
    }

    st.block += 1;
    Ok(())
}

/// 发送文件传输请求
struct SendRequest<'a> {
    sock: &'a Socket,
    buf: &'a mut [u8],
    msg: Message,
}

impl<'a> SendRequest<'a> {
    fn new(sock: &'a Socket, buf: &'a mut [u8], name: String, size: u64) -> Self {
        let resume = true;
        let msg = Message::Request(Request::new(name, size, resume));
        Self { sock, buf, msg }
    }
}

#[async_trait]
impl<'a> Operation<Option<Response>> for SendRequest<'a> {
    async fn poll(&mut self) -> std::io::Result<()> {
        self.sock.send(&self.msg).await
    }

    async fn resolve(&mut self) -> std::io::Result<Option<Response>> {
        loop {
            match self.sock.recv(self.buf).await? {
                Message::Response(response) => return Ok(Some(response)),
                Message::FileComplete => return Ok(None),
                _ => {}
            }
        }
    }
}

/// 发送分块完成消息
struct SendBlockComplete<'a> {
    sock: &'a Socket,
    buf: &'a mut [u8],
    block: u32,
    missing_chunk: Option<Vec<u32>>,
}

impl<'a> SendBlockComplete<'a> {
    fn new(sock: &'a Socket, buf: &'a mut [u8], block: u32) -> Self {
        Self {
            sock,
            buf,
            block,
            missing_chunk: None,
        }
    }
}

#[async_trait]
impl<'a> Operation<Vec<u32>> for SendBlockComplete<'a> {
    async fn poll(&mut self) -> std::io::Result<()> {
        self.sock.send(&Message::BlockComplete(self.block)).await
    }

    async fn resolve(&mut self) -> std::io::Result<Vec<u32>> {
        loop {
            match self.sock.recv(&mut self.buf).await? {
                Message::BlockCompleteAck(block) if block == self.block => return Ok(vec![]),
                Message::BlockMissingChunk {
                    block,
                    chunk,
                    count,
                } if block == self.block => {
                    let v = self.missing_chunk.get_or_insert(vec![]);
                    v.extend_from_slice(&chunk);
                    if v.len() == count as usize {
                        return Ok(self.missing_chunk.take().unwrap());
                    }
                }
                _ => {}
            }
        }
    }

    fn result(&mut self) -> Option<Vec<u32>> {
        self.missing_chunk.take()
    }
}

#[derive(Debug)]
struct Statistic {
    block: u64,
    chunk: u64,
    resend_chunk: u64,
    time: Instant,
}

impl Default for Statistic {
    fn default() -> Self {
        Self {
            block: 0,
            chunk: 0,
            resend_chunk: 0,
            time: Instant::now(),
        }
    }
}

impl Display for Statistic {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let time = self.time.elapsed().as_millis();
        write!(
            f,
            "statistic: block {} chunk {} resend_chunk {} time {}ms",
            self.block, self.chunk, self.resend_chunk, time
        )
    }
}
