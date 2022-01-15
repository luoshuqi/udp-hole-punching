use std::io::{self, ErrorKind};
use std::path::PathBuf;

use async_trait::async_trait;
use log::{debug, info};
use tokio::time::{sleep, Duration};

use crate::file_transfer::block::BlockWriter;
use crate::file_transfer::message::Request;
use crate::file_transfer::{Message, Response};
use crate::{perform, Operation, Socket};

/// block 大小为 1 MiB
const BLOCK_SIZE: u32 = 1048576;

/// chunk 大小为 496 bytes = 576 (IP 最小 MTU) - 60 (IP 最大头部) - 8 (UDP 头部) - 12 (Message::Chunk 大小)
const CHUNK_SIZE: u16 = 496;

/// Message::Chunk 大小
const CHUNK_HEAD_SIZE: usize = 12;

/// 读取超时时间
const READ_TIMEOUT: u64 = 5;

/// 接收文件
pub async fn receive(sock: Socket, path: PathBuf) -> crate::Result<()> {
    let mut buf = vec![0u8; CHUNK_HEAD_SIZE + CHUNK_SIZE as usize];

    let req: Request = tokio::select! {
        req = read_request(&sock, &mut buf) => {
            req?
        }
        _ = sleep(Duration::from_secs(READ_TIMEOUT)) => {
            Err(io::Error::from(ErrorKind::TimedOut)).map_err(err!())?
        }
    };

    info!("receiving {}", req.name);

    let mut writer =
        match BlockWriter::new(path.join(&req.name), req.size, BLOCK_SIZE, CHUNK_SIZE, true)? {
            Some(v) => v,
            None => return complete(&sock, &mut buf, &req.name).await,
        };

    let mut block = writer.next_block().unwrap();
    let mut op = SendResponse {
        sock: &sock,
        buf: &mut buf,
        block: block.index(),
    };
    let first_chunk = perform(&mut op).await.map_err(err!())?;
    block.write(first_chunk.chunk, &buf[first_chunk.start..first_chunk.end]);

    loop {
        tokio::select! {
            msg = read_message(&sock, &mut buf) => {
                match msg.map_err(err!())? {
                    (Message::FilePart { block: b, chunk }, data) if b == block.index() => block.write(chunk, data),
                    (Message::BlockComplete(b), _) if b == block.index() => {
                        let missing = block.get_missing_chunk();
                        if missing.is_empty() {
                            sock.send(&Message::BlockCompleteAck(b)).await.map_err(err!())?;
                            block.commit()?;
                            block = match writer.next_block() {
                                Some(v) => v,
                                None => break,
                            };
                        } else {
                            let count = missing.len() as u32;
                            for v in missing.as_slice().chunks(100) {
                                let msg = Message::BlockMissingChunk { block: b, chunk: v.to_vec(), count };
                                sock.send(&msg).await.map_err(err!())?;
                            }
                        }
                    }
                    // 发送端未收到 Message::BlockCompleteAck(b)
                    (Message::BlockComplete(b), _) if b + 1 == block.index() => {
                        sock.send(&Message::BlockCompleteAck(b)).await.map_err(err!())?;
                    }
                    _ => {}
                }
            }
            _ = sleep(Duration::from_secs(READ_TIMEOUT)) => {
                Err(io::Error::from(ErrorKind::TimedOut)).map_err(err!())?
            }
        }
    }

    writer.rename_file()?;
    complete(&sock, &mut buf, &req.name).await
}

/// 读取发送请求
async fn read_request(sock: &Socket, buf: &mut [u8]) -> crate::Result<Request> {
    loop {
        if let Message::Request(req) = sock.recv(buf).await.map_err(err!())? {
            return Ok(req);
        }
    }
}

async fn read_message<'a>(sock: &Socket, buf: &'a mut [u8]) -> io::Result<(Message, &'a [u8])> {
    loop {
        let n = sock.as_ref().recv(buf).await?;
        if let Some((msg, remain)) = Message::trailing_decode(&buf[..n]) {
            let addr = sock.connected_addr().unwrap();
            debug!("receive {:?} from {}", msg, addr);

            match msg {
                Message::FilePart { .. } => return Ok((msg, &buf[n - remain..n])),
                msg if remain == 0 => return Ok((msg, &[])),
                _ => {}
            }
        }
    }
}

async fn complete(sock: &Socket, buf: &mut [u8], filename: &str) -> crate::Result<()> {
    let mut op = SendComplete { sock, buf };
    perform(&mut op).await.map_err(err!())?;
    info!("receive {} complete", filename);
    Ok(())
}

/// 发送传输完成消息
struct SendComplete<'a> {
    sock: &'a Socket,
    buf: &'a mut [u8],
}

#[async_trait]
impl<'a> Operation<()> for SendComplete<'a> {
    async fn poll(&mut self) -> io::Result<()> {
        self.sock.send(&Message::FileComplete).await
    }

    async fn resolve(&mut self) -> io::Result<()> {
        loop {
            if let Message::FileCompleteAck = self.sock.recv(self.buf).await? {
                return Ok(());
            }
        }
    }

    fn result(&mut self) -> Option<()> {
        Some(())
    }
}

/// 发送响应消息
struct SendResponse<'a> {
    sock: &'a Socket,
    buf: &'a mut [u8],
    block: u32,
}

struct FirstChunk {
    chunk: u32,
    start: usize,
    end: usize,
}

#[async_trait]
impl<'a> Operation<FirstChunk> for SendResponse<'a> {
    async fn poll(&mut self) -> io::Result<()> {
        let resp = Response::new(BLOCK_SIZE, CHUNK_SIZE, self.block);
        self.sock.send(&Message::Response(resp)).await
    }

    async fn resolve(&mut self) -> io::Result<FirstChunk> {
        loop {
            let n = self.sock.as_ref().recv(self.buf).await?;
            if let Some((msg, remain)) = Message::trailing_decode(&self.buf[..n]) {
                let addr = self.sock.connected_addr().unwrap();
                debug!("receive {:?} from {}", msg, addr);
                match msg {
                    Message::FilePart { block, chunk } if block == self.block => {
                        return Ok(FirstChunk {
                            chunk,
                            start: n - remain,
                            end: n,
                        });
                    }
                    _ => {}
                }
            }
        }
    }
}
