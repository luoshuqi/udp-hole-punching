use std::net::SocketAddr;

use serde::{Deserialize, Serialize};

use crate::{Decode, Encode};

#[derive(Serialize, Deserialize, Debug)]
pub enum Message {
    /// peer 向外网服务器查询自己的外网地址
    Query,

    /// 外网服务器回复 peer 其外网地址
    Address(SocketAddr),

    /// peer 向外网服务器注册, 其他 peer 可通过 id 连接此 peer
    Register { id: Vec<u8> },

    /// 注册确认
    RegisterAck,

    /// peer 向外网服务器查询另一个 peer 的外网地址
    Lookup { peer_id: Vec<u8> },

    /// 外网服务器回复 peer 查询结果
    Peer { addr: Option<SocketAddr> },

    /// 外网服务器通知 peer 有其他 peer 想要获取其外网地址
    Request {
        peer_addr: SocketAddr, // 发起查询的 peer 的外网地址
    },

    /// peer 通知外网服务器使用当前 socket 的地址作为其外网地址
    Response {
        peer_addr: SocketAddr, // 发起查询的 peer 的外网地址
    },

    /// Response 确认
    ResponseAck,

    /// 打洞消息
    Hello,

    /// Hello 确认
    HelloAck,
}

/// 附加到消息结尾，防止把来自其它地址的非 Message 数据当作 Message 处理
const MAGIC: &[u8; 8] = &[205, 126, 86, 230, 111, 189, 169, 142];

impl Encode for Message {
    fn encode(&self) -> Vec<u8> {
        let cap = bincode::serialized_size(self).unwrap() as usize + MAGIC.len();
        let mut v = Vec::with_capacity(cap);
        bincode::serialize_into(&mut v, self).unwrap();
        v.extend_from_slice(MAGIC);
        v
    }
}

impl Decode for Message {
    fn decode(mut data: &[u8]) -> Option<Self> {
        let msg = bincode::deserialize_from(&mut data).ok()?;
        if data == MAGIC {
            Some(msg)
        } else {
            None
        }
    }
}
