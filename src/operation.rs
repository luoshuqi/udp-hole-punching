use std::io::{self, ErrorKind};
use std::time::Duration;

use async_trait::async_trait;
use tokio::time::sleep;

/// 超时重试操作
#[async_trait]
pub trait Operation<T> {
    /// 重试次数
    const RETRY_COUNT: usize = 3;

    /// 超时时间
    const RETRY_DURATION: Duration = Duration::from_millis(150);

    /// 执行操作
    async fn poll(&mut self) -> io::Result<()>;

    /// 处理结果
    async fn resolve(&mut self) -> io::Result<T>;

    /// 操作超时后获取结果
    fn result(&mut self) -> Option<T> {
        None
    }
}

/// 执行超时重试操作
pub async fn perform<T, U>(operation: &mut T) -> io::Result<U>
where
    T: Operation<U>,
{
    operation.poll().await?;

    let mut attempt = 0;
    loop {
        tokio::select! {
            v = operation.resolve() => {
                return v;
            }
            _ = sleep(T::RETRY_DURATION) => {
                if attempt < T::RETRY_COUNT {
                    attempt += 1;
                    operation.poll().await?;
                } else {
                    return match operation.result() {
                        Some(v) => Ok(v),
                        None => Err(io::Error::from(ErrorKind::TimedOut)),
                    }
                }
            }
        }
    }
}
