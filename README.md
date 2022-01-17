# udp-hole-punching

#### 介绍
UDP 打洞，文件传输

### 构建

```shell
cargo build --release
```

`bin` 目录有编译好的适用于 `x86_64 linux` 的可执行文件
（在 `Debian 11 和 Ubuntu 20.04` 上测试过）。

#### 使用说明

1. 在外网服务器上运行 `server`:

```shell
./server --addr 0.0.0.0:4567 --addr2 0.0.0.0:6789
```

`server` 的作用是供 peer 查询外网地址，协调打洞。 绑定2个地址，peer 可以探测自己是否处在对称型 NAT 后面。

2. 运行接收端（假设外网服务器的域名为 foo.com）:
```shell
./peer --addr foo.com:4567 --addr2 foo.com:6789 --id bar --receive /tmp
```

- `id` 指定 peer 标识，一个 peer 可通过 id 找到其它 peer
- `receive` 指定接收文件保存目录

3. 执行发送端
```shell
./peer --addr foo.com:4567 --addr2 foo.com:6789 --id bar --send /data/test
```

- `id` 指定发送端 id
- `send` 指定发送的文件

如果不是对称型 NAT 而打洞失败，可重试几次。