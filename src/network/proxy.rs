//! HAProxy PROXY 协议解析（可选，文档 2.1 节）。
//! 支持 v1（文本）与 v2（二进制）。解析成功后可能剩余字节（含协议版本字节），
//! 需回灌给握手阶段。

use std::net::SocketAddr;
use tokio::io::AsyncReadExt;
use tokio::net::TcpStream;
use tokio::time::timeout;

const PROXY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// PROXY 解析结果。
pub struct ProxyResult {
    /// 真实客户端地址。
    pub real_addr: Option<SocketAddr>,
    /// 解析后剩余的字节（可能包含协议版本字节，需先消费）。
    pub pending: Vec<u8>,
}

/// v2 签名：\r\n\r\n\0\r\nQUIT\n
const V2_SIG: &[u8] = b"\r\n\r\n\x00\r\nQUIT\n";

pub async fn parse_proxy(stream: &mut TcpStream) -> std::io::Result<ProxyResult> {
    timeout(PROXY_TIMEOUT, parse_inner(stream))
        .await
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "proxy protocol timeout"))?
}

async fn parse_inner(stream: &mut TcpStream) -> std::io::Result<ProxyResult> {
    // 先读 16 字节判断版本
    let mut head = [0u8; 16];
    let n = stream.read(&mut head).await?;
    if n == 0 {
        return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "eof"));
    }

    if head.starts_with(V2_SIG) {
        parse_v2(stream, head, n).await
    } else if head.starts_with(b"PROXY") {
        parse_v1(stream, head, n).await
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "bad proxy protocol header",
        ))
    }
}

async fn parse_v1(stream: &mut TcpStream, head: [u8; 16], n: usize) -> std::io::Result<ProxyResult> {
    // v1: "PROXY TCP4 src dst sport dport\r\n"
    let mut line = head[..n].to_vec();
    while !line.windows(2).any(|w| w == b"\r\n") {
        let mut b = [0u8; 1];
        stream.read_exact(&mut b).await?;
        line.push(b[0]);
        if line.len() > 108 {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "proxy v1 line too long"));
        }
    }
    let crlf = line.windows(2).position(|w| w == b"\r\n").unwrap();
    let pending = line[crlf + 2..].to_vec();
    let text = String::from_utf8_lossy(&line[..crlf]).to_string();
    let parts: Vec<&str> = text.split(' ').collect();
    // PROXY TCP4 1.2.3.4 5.6.7.8 1234 12346
    let real_addr = if parts.len() >= 6 && (parts[1] == "TCP4" || parts[1] == "TCP6") {
        format!("{}:{}", parts[2], parts[4]).parse().ok()
    } else {
        None
    };
    Ok(ProxyResult { real_addr, pending })
}

async fn parse_v2(stream: &mut TcpStream, head: [u8; 16], n: usize) -> std::io::Result<ProxyResult> {
    // v2: sig(12) ver/cmd(1) fam/proto(1) len(2) [addr...]
    let mut hdr = [0u8; 4];
    hdr.copy_from_slice(&head[12..16]);
    let _ver_cmd = hdr[0];
    let fam_proto = hdr[1];
    let len = u16::from_be_bytes([hdr[2], hdr[3]]) as usize;

    let mut extra = head[16..n].to_vec();
    while extra.len() < len {
        let mut b = vec![0u8; len - extra.len()];
        let m = stream.read(&mut b).await?;
        if m == 0 {
            return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "eof"));
        }
        extra.extend_from_slice(&b[..m]);
    }
    let pending = extra.split_off(len.min(extra.len()));

    let real_addr = match fam_proto {
        0x11 if len >= 12 => {
            // TCP over IPv4: src(4) dst(4) sport(2) dport(2)
            let ip = std::net::Ipv4Addr::new(extra[0], extra[1], extra[2], extra[3]);
            let port = u16::from_be_bytes([extra[8], extra[9]]);
            Some(SocketAddr::new(ip.into(), port))
        }
        0x21 if len >= 36 => {
            // TCP over IPv6
            let mut ip = [0u8; 16];
            ip.copy_from_slice(&extra[0..16]);
            let port = u16::from_be_bytes([extra[32], extra[33]]);
            Some(SocketAddr::new(std::net::Ipv6Addr::from(ip).into(), port))
        }
        _ => None,
    };
    Ok(ProxyResult { real_addr, pending })
}
