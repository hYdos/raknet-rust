# mcbe-raknet-rs

[![Rust](https://img.shields.io/badge/Rust-2024_edition-000000?logo=rust)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/License-Apache--2.0-blue.svg)](LICENSE)
[![Status](https://img.shields.io/badge/Status-Alpha-orange)](#)
[![Platform](https://img.shields.io/badge/Platform-%20RakNet-2ea44f)](#)

`mcbe-raknet-rs` is a RakNet library written in Rust for Minecraft Bedrock Edition.


## Installation

```toml
[dependencies]
mcbe-raknet-rs = { path = "../mcbe-raknet-rs" }
```

## Usage

### Basic Server

```rust
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use mcbe_raknet_rs::server::{RaknetServer, RaknetServerEvent};

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let bind = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 19132);
    let mut server = RaknetServer::bind(bind).await?;

    while let Some(event) = server.next_event().await {
        if let RaknetServerEvent::Packet { peer_id, payload, .. } = event {
            server.send(peer_id, payload).await?;
        }
    }

    Ok(())
}
```

### Listener Facade (accept/send/recv/close/metadata)

```rust
use std::net::SocketAddr;
use mcbe_raknet_rs::listener::Listener;
use mcbe_raknet_rs::connection::RecvError;

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let bind = SocketAddr::from(([0, 0, 0, 0], 19132));
    let mut listener = Listener::bind(bind).await?;
    listener.start().await?;

    loop {
        let mut conn = listener.accept().await?;
        let meta = conn.metadata();
        println!("peer={} addr={}", meta.id().as_u64(), meta.remote_addr());

        tokio::spawn(async move {
            loop {
                match conn.recv_bytes().await {
                    Ok(payload) => {
                        if conn.send_bytes(payload).await.is_err() {
                            break;
                        }
                    }
                    Err(RecvError::ConnectionClosed { .. }) | Err(RecvError::ChannelClosed) => break,
                    Err(RecvError::DecodeError { .. }) => {}
                }
            }
        });
    }
}
```

### Basic Client

```rust
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use mcbe_raknet_rs::client::{RaknetClient, RaknetClientEvent};

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 19132);
    let mut client = RaknetClient::connect(addr).await?;

    while let Some(event) = client.next_event().await {
        match event {
            RaknetClientEvent::Connected { .. } => client.send(b"\xfehello").await?,
            RaknetClientEvent::Packet { .. } => break,
            RaknetClientEvent::Disconnected { .. } => break,
            _ => {}
        }
    }

    Ok(())
}
```

## Development

```bash
cargo fmt
cargo test -q
cargo check --examples -q
```

Soak example:

```bash
cargo run --example raknet_soak -- --sessions=512 --ticks=2000 --payload-bytes=180
```

Listener facade echo/proxy example:

```bash
cargo run --example listener_facade -- --listen 0.0.0.0:19132
cargo run --example listener_facade -- --listen 0.0.0.0:19132 --upstream 127.0.0.1:19133
```

## License

Apache-2.0. See [LICENSE](LICENSE) for details.
