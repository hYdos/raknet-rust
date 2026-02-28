# raknet-rust

[![Rust](https://img.shields.io/badge/Rust-2024_edition-000000?logo=rust)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/License-Apache--2.0-blue.svg)](LICENSE)
[![Status](https://img.shields.io/badge/Status-Alpha-orange)](#)
[![Platform](https://img.shields.io/badge/Platform-%20RakNet-2ea44f)](#)

`raknet-rust` is a RakNet transport library written in Rust.

It is built for modern async server/client networking and is especially useful for
Minecraft Bedrock Edition projects (servers, proxies, and tooling), while still remaining
usable as a general RakNet library.

## Getting Started

### Installation

```toml
[dependencies]
raknet-rust = { path = "../raknet-rust" }
```

### Usage

Basic server:

```rust
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use raknet_rust::server::{RaknetServer, RaknetServerEvent};

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

Basic client:

```rust
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use raknet_rust::client::{RaknetClient, RaknetClientEvent};

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

## License

Apache-2.0. See [LICENSE](LICENSE) for details.
