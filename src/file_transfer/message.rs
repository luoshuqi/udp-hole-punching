use std::fmt::{Debug, Formatter};

use bincode::{DefaultOptions, Options};
use serde::{Deserialize, Serialize};

use crate::{Decode, Encode};

#[derive(Serialize, Deserialize, Debug)]
pub struct Request {
    /// 文件名
    pub name: String,
    /// 文件大小
    pub size: u64,
    /// 断点续传
    pub resume: bool,
}

impl Request {
    pub fn new(name: String, size: u64, resume: bool) -> Self {
        Self { name, size, resume }
    }
}

#[derive(Serialize, Deserialize, Debug, Copy, Clone)]
pub struct Response {
    /// block 大小
    ///
    /// 文件分 block，每个 block 确认收到后才发送下一个 block
    pub block_size: u32,
    /// chunk 大小
    ///
    /// block 分 chunk, 一个 chunk 通过一个 UDP 包发送
    pub chunk_size: u16,
    /// 断点续传位置
    pub start_block: u32,
}

impl Response {
    pub fn new(block_size: u32, chunk_size: u16, start_block: u32) -> Self {
        Self {
            block_size,
            chunk_size,
            start_block,
        }
    }
}

/// 文件传输消息
#[derive(Serialize, Deserialize, Debug)]
pub enum Message {
    /// 发送端请求发送文件
    Request(Request),

    /// 接收端确认接收文件
    Response(Response),

    /// 文件分块数据
    FilePart {
        /// chunk 所属 block
        block: u32,
        /// chunk 在 block 中的位置
        chunk: u32,
        // 文件数据附加在消息之后，不参与序列化，不然影响性能
    },

    /// 发送端通知 block 发送完毕
    BlockComplete(u32),

    /// 接收端确认 block 已完整接收
    BlockCompleteAck(u32),

    /// 接收端指示缺少 chunk，发送端需要重发
    BlockMissingChunk {
        block: u32,
        chunk: Vec<u32>,
        count: u32, // 缺少的 chunk 个数
    },

    /// 接收端通知文件已接收
    FileComplete,

    /// 发送端确认收到 File 消息
    FileCompleteAck,
}

impl Message {
    /// 返回消息和剩余数据的长度
    pub fn trailing_decode(mut data: &[u8]) -> Option<(Message, usize)> {
        let msg = bincode::deserialize_from(&mut data).ok()?;
        Some((msg, data.len()))
    }
}

impl Encode for Message {
    fn encode(&self) -> Vec<u8> {
        bincode::serialize(self).unwrap()
    }
}

impl Decode for Message {
    fn decode(data: &[u8]) -> Option<Self> {
        // 发送文件时，peer 已互相连接对方，不会收到来自其它地址的数据，所以不做额外验证
        DefaultOptions::new()
            .with_fixint_encoding()
            .reject_trailing_bytes()
            .deserialize(data)
            .ok()
    }
}

/// 文件分块
pub struct Chunk<'a> {
    header: Message,
    data: &'a [u8],
}

impl<'a> Chunk<'a> {
    pub fn new(block: u32, chunk: u32, data: &'a [u8]) -> Self {
        Self {
            header: Message::FilePart { block, chunk },
            data,
        }
    }
}

impl<'a> Debug for Chunk<'a> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        Debug::fmt(&self.header, f)
    }
}

impl<'a> Encode for Chunk<'a> {
    fn encode(&self) -> Vec<u8> {
        let mut v = self.header.encode();
        v.extend_from_slice(self.data);
        v
    }
}
