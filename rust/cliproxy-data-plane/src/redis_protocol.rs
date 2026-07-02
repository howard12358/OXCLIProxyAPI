use anyhow::Result;
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, ReadHalf, WriteHalf, split},
    net::TcpStream,
};

use crate::usage_queue::{UsageQueue, UsageSubscription};

/// 判断一条 TCP 连接是否应按 Redis RESP 协议处理。
pub fn is_resp_prefix(prefix: u8) -> bool {
    matches!(prefix, b'*' | b'$' | b'+' | b'-' | b':')
}

/// 处理 CPA 兼容的 Redis-like usage queue 协议。
///
/// 当前只实现 Keeper 需要的最小命令集，不尝试成为完整 Redis server。
pub async fn handle_connection(
    stream: TcpStream,
    usage_queue: UsageQueue,
    auth_password: Option<String>,
) -> Result<()> {
    let (reader, mut writer) = split(stream);
    let mut reader = BufReader::new(reader);
    let mut authed = false;

    loop {
        let args = match read_resp_array(&mut reader).await {
            Ok(args) => args,
            Err(_) => return Ok(()),
        };
        if args.is_empty() {
            write_error(&mut writer, "ERR empty command").await?;
            continue;
        }

        let command = args[0].trim().to_ascii_uppercase();
        if command != "AUTH" && !authed {
            write_error(&mut writer, "NOAUTH Authentication required.").await?;
            continue;
        }

        match command.as_str() {
            "AUTH" => {
                let Some(password) = parse_auth_password(&args) else {
                    write_error(
                        &mut writer,
                        "ERR wrong number of arguments for 'auth' command",
                    )
                    .await?;
                    continue;
                };
                if let Some(expected) = auth_password.as_deref()
                    && password != expected
                {
                    write_error(&mut writer, "ERR invalid password").await?;
                    continue;
                }
                authed = true;
                write_simple_string(&mut writer, "OK").await?;
            }
            "SUBSCRIBE" => {
                if args.len() != 2 {
                    write_error(
                        &mut writer,
                        "ERR wrong number of arguments for 'subscribe' command",
                    )
                    .await?;
                    continue;
                }
                let channel = args[1].trim().to_ascii_lowercase();
                let subscription = match channel.as_str() {
                    "usage" => usage_queue.subscribe_usage(),
                    "errors" => usage_queue.subscribe_errors(),
                    _ => {
                        write_error(&mut writer, &format!("ERR unsupported channel '{channel}'"))
                            .await?;
                        continue;
                    }
                };
                write_pubsub_subscribe(&mut writer, &channel, 1).await?;
                stream_subscription(&mut reader, &mut writer, &channel, subscription).await?;
                return Ok(());
            }
            "LPOP" | "RPOP" => {
                let (channel, count, has_count) = match parse_pop_args(&args) {
                    Some(value) => value,
                    None => {
                        write_error(
                            &mut writer,
                            &format!(
                                "ERR wrong number of arguments for '{}' command",
                                command.to_ascii_lowercase()
                            ),
                        )
                        .await?;
                        continue;
                    }
                };
                if channel != "usage" {
                    write_error(&mut writer, &format!("ERR unsupported channel '{channel}'"))
                        .await?;
                    continue;
                }
                if count == 0 {
                    write_error(&mut writer, "ERR value is not an integer or out of range").await?;
                    continue;
                }
                let items = usage_queue.pop_oldest(count);
                if has_count {
                    write_bulk_array(&mut writer, &items).await?;
                } else if let Some(item) = items.first() {
                    write_bulk_string(&mut writer, item).await?;
                } else {
                    write_nil_bulk_string(&mut writer).await?;
                }
            }
            _ => {
                write_error(
                    &mut writer,
                    &format!("ERR unknown command '{}'", command.to_ascii_lowercase()),
                )
                .await?;
            }
        }
    }
}

async fn stream_subscription(
    reader: &mut BufReader<ReadHalf<TcpStream>>,
    writer: &mut WriteHalf<TcpStream>,
    channel: &str,
    mut subscription: UsageSubscription,
) -> Result<()> {
    loop {
        tokio::select! {
            payload = subscription.recv() => {
                let Some(payload) = payload else {
                    return Ok(());
                };
                write_pubsub_message(writer, channel, &payload).await?;
            }
            command = read_resp_array(reader) => {
                let Ok(args) = command else {
                    return Ok(());
                };
                if args.is_empty() {
                    write_error(writer, "ERR empty command").await?;
                    continue;
                }
                match args[0].trim().to_ascii_uppercase().as_str() {
                    "PING" => {
                        let payload = args.get(1).map(String::as_bytes).unwrap_or_default();
                        write_pubsub_pong(writer, payload).await?;
                    }
                    "UNSUBSCRIBE" => {
                        write_pubsub_unsubscribe(writer, channel, 0).await?;
                        return Ok(());
                    }
                    "QUIT" => {
                        write_simple_string(writer, "OK").await?;
                        return Ok(());
                    }
                    other => {
                        write_error(writer, &format!("ERR unknown command '{}'", other.to_ascii_lowercase())).await?;
                    }
                }
            }
        }
    }
}

fn parse_auth_password(args: &[String]) -> Option<&str> {
    match args.len() {
        2 => Some(args[1].as_str()),
        3 => Some(args[2].as_str()),
        _ => None,
    }
}

fn parse_pop_args(args: &[String]) -> Option<(String, usize, bool)> {
    if args.len() != 2 && args.len() != 3 {
        return None;
    }
    let channel = args[1].trim().to_ascii_lowercase();
    if args.len() == 2 {
        return Some((channel, 1, false));
    }
    let count = args[2].trim().parse::<usize>().ok()?;
    Some((channel, count, true))
}

async fn read_resp_array(reader: &mut BufReader<ReadHalf<TcpStream>>) -> Result<Vec<String>> {
    let mut prefix = [0; 1];
    reader.read_exact(&mut prefix).await?;
    anyhow::ensure!(prefix[0] == b'*', "protocol error");
    let count = read_resp_line(reader).await?.parse::<usize>()?;
    let mut args = Vec::with_capacity(count);
    for _ in 0..count {
        args.push(read_resp_string(reader).await?);
    }
    Ok(args)
}

async fn read_resp_string(reader: &mut BufReader<ReadHalf<TcpStream>>) -> Result<String> {
    let mut prefix = [0; 1];
    reader.read_exact(&mut prefix).await?;
    match prefix[0] {
        b'$' => read_resp_bulk_string(reader).await,
        b'+' | b':' => read_resp_line(reader).await,
        _ => anyhow::bail!("protocol error"),
    }
}

async fn read_resp_bulk_string(reader: &mut BufReader<ReadHalf<TcpStream>>) -> Result<String> {
    let len = read_resp_line(reader).await?.parse::<isize>()?;
    if len < 0 {
        return Ok(String::new());
    }
    let len = len as usize;
    let mut buf = vec![0; len + 2];
    reader.read_exact(&mut buf).await?;
    anyhow::ensure!(buf[len] == b'\r' && buf[len + 1] == b'\n', "protocol error");
    Ok(String::from_utf8_lossy(&buf[..len]).into_owned())
}

async fn read_resp_line(reader: &mut BufReader<ReadHalf<TcpStream>>) -> Result<String> {
    let mut line = String::new();
    reader.read_line(&mut line).await?;
    Ok(line.trim_end_matches(['\r', '\n']).to_string())
}

async fn write_simple_string(writer: &mut WriteHalf<TcpStream>, value: &str) -> Result<()> {
    writer.write_all(format!("+{value}\r\n").as_bytes()).await?;
    Ok(())
}

async fn write_error(writer: &mut WriteHalf<TcpStream>, value: &str) -> Result<()> {
    writer.write_all(format!("-{value}\r\n").as_bytes()).await?;
    Ok(())
}

async fn write_nil_bulk_string(writer: &mut WriteHalf<TcpStream>) -> Result<()> {
    writer.write_all(b"$-1\r\n").await?;
    Ok(())
}

async fn write_bulk_string(writer: &mut WriteHalf<TcpStream>, value: &[u8]) -> Result<()> {
    writer
        .write_all(format!("${}\r\n", value.len()).as_bytes())
        .await?;
    writer.write_all(value).await?;
    writer.write_all(b"\r\n").await?;
    Ok(())
}

async fn write_bulk_array(writer: &mut WriteHalf<TcpStream>, items: &[Vec<u8>]) -> Result<()> {
    writer
        .write_all(format!("*{}\r\n", items.len()).as_bytes())
        .await?;
    for item in items {
        write_bulk_string(writer, item).await?;
    }
    Ok(())
}

async fn write_array_header(writer: &mut WriteHalf<TcpStream>, count: usize) -> Result<()> {
    writer.write_all(format!("*{count}\r\n").as_bytes()).await?;
    Ok(())
}

async fn write_integer(writer: &mut WriteHalf<TcpStream>, value: usize) -> Result<()> {
    writer.write_all(format!(":{value}\r\n").as_bytes()).await?;
    Ok(())
}

async fn write_pubsub_subscribe(
    writer: &mut WriteHalf<TcpStream>,
    channel: &str,
    count: usize,
) -> Result<()> {
    write_array_header(writer, 3).await?;
    write_bulk_string(writer, b"subscribe").await?;
    write_bulk_string(writer, channel.as_bytes()).await?;
    write_integer(writer, count).await
}

async fn write_pubsub_unsubscribe(
    writer: &mut WriteHalf<TcpStream>,
    channel: &str,
    count: usize,
) -> Result<()> {
    write_array_header(writer, 3).await?;
    write_bulk_string(writer, b"unsubscribe").await?;
    write_bulk_string(writer, channel.as_bytes()).await?;
    write_integer(writer, count).await
}

async fn write_pubsub_message(
    writer: &mut WriteHalf<TcpStream>,
    channel: &str,
    payload: &[u8],
) -> Result<()> {
    write_array_header(writer, 3).await?;
    write_bulk_string(writer, b"message").await?;
    write_bulk_string(writer, channel.as_bytes()).await?;
    write_bulk_string(writer, payload).await
}

async fn write_pubsub_pong(writer: &mut WriteHalf<TcpStream>, payload: &[u8]) -> Result<()> {
    write_array_header(writer, 2).await?;
    write_bulk_string(writer, b"pong").await?;
    write_bulk_string(writer, payload).await
}

#[cfg(test)]
mod tests {
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::{TcpListener, TcpStream},
    };

    use crate::usage_queue::UsageQueue;

    use super::{handle_connection, is_resp_prefix};

    #[test]
    fn detects_resp_prefixes() {
        assert!(is_resp_prefix(b'*'));
        assert!(is_resp_prefix(b'$'));
        assert!(!is_resp_prefix(b'G'));
    }

    #[tokio::test]
    async fn lpop_returns_buffered_usage_payload() {
        let queue = UsageQueue::new();
        queue.enqueue_raw(br#"{"request_id":"req-1"}"#.to_vec());
        let addr = spawn_redis_protocol(queue, None).await;
        let mut client = TcpStream::connect(addr).await.expect("connect");

        client
            .write_all(
                b"*2\r\n$4\r\nAUTH\r\n$6\r\nsecret\r\n*3\r\n$4\r\nLPOP\r\n$5\r\nusage\r\n$1\r\n1\r\n",
            )
            .await
            .expect("write command");
        let mut buf = vec![0; 128];
        let n = client.read(&mut buf).await.expect("read response");
        let mut response = String::from_utf8_lossy(&buf[..n]).into_owned();
        if !response.contains(r#"{"request_id":"req-1"}"#) {
            let n = client.read(&mut buf).await.expect("read response");
            response.push_str(&String::from_utf8_lossy(&buf[..n]));
        }

        assert!(response.contains("+OK\r\n"), "response: {response:?}");
        assert!(
            response.contains(r#"{"request_id":"req-1"}"#),
            "response: {response:?}"
        );
    }

    #[tokio::test]
    async fn subscribe_usage_emits_support_refresh_control_message() {
        let addr = spawn_redis_protocol(UsageQueue::new(), None).await;
        let mut client = TcpStream::connect(addr).await.expect("connect");

        client
            .write_all(
                b"*2\r\n$4\r\nAUTH\r\n$6\r\nsecret\r\n*2\r\n$9\r\nSUBSCRIBE\r\n$5\r\nusage\r\n",
            )
            .await
            .expect("write command");
        let mut buf = vec![0; 256];
        let n = client.read(&mut buf).await.expect("read response");
        let mut response = String::from_utf8_lossy(&buf[..n]).into_owned();
        while !response.contains(r#"{"support_refresh":true}"#) {
            let n = client.read(&mut buf).await.expect("read response");
            if n == 0 {
                break;
            }
            response.push_str(&String::from_utf8_lossy(&buf[..n]));
        }

        assert!(response.contains("subscribe"), "response: {response:?}");
        assert!(
            response.contains(r#"{"support_refresh":true}"#),
            "response: {response:?}"
        );
    }

    #[tokio::test]
    async fn auth_rejects_wrong_password_when_configured() {
        let addr = spawn_redis_protocol(UsageQueue::new(), Some("secret".to_string())).await;
        let mut client = TcpStream::connect(addr).await.expect("connect");

        client
            .write_all(b"*2\r\n$4\r\nAUTH\r\n$5\r\nwrong\r\n")
            .await
            .expect("write command");
        let mut buf = vec![0; 128];
        let n = client.read(&mut buf).await.expect("read response");
        let response = String::from_utf8_lossy(&buf[..n]);

        assert!(
            response.contains("-ERR invalid password"),
            "response: {response:?}"
        );
    }

    async fn spawn_redis_protocol(
        queue: UsageQueue,
        auth_password: Option<String>,
    ) -> std::net::SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept");
            handle_connection(stream, queue, auth_password)
                .await
                .expect("handle connection");
        });
        addr
    }
}
