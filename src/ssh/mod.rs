use std::{
    convert::Infallible,
    sync::{
        atomic::{self, AtomicUsize},
        Arc,
    },
};

use color_eyre::Result;
use russh::server::{self};
use russh_keys::key;
use tokio::{net::TcpListener, sync::broadcast};
use tracing::{error, info};

use crate::ssh::content::SshContent;

use self::session::SshSession;

mod apps;
mod content;
mod session;
mod terminal;

pub async fn main(_rx: broadcast::Receiver<()>) -> Result<Infallible> {
    // TODO: add live-reload when we get message from _rx
    // Setup content, config, and listener
    let content = Arc::new(SshContent::new(&crate::CONTENT.read().unwrap()));
    let mut config = server::Config::default();
    config.keys = vec![key::KeyPair::Ed25519(
        ed25519_dalek::Keypair::from_bytes(crate::CONFIG.ssh_key.to_bytes().as_ref()).unwrap(),
    )];
    let config = Arc::new(config);
    let listener = TcpListener::bind(("0.0.0.0", 23)).await?;

    // Setup connection handling, initializing all necessary variables (could later include Vec of all connections and connection time or other load-managing stuff)
    let active_connections = Arc::new(AtomicUsize::new(0));
    let total_connections: AtomicUsize = AtomicUsize::new(0);

    // Run server
    info!("Starting SSH Server...");

    loop {
        let (stream, addr) = listener.accept().await?;
        let conn_id = total_connections.fetch_add(1, atomic::Ordering::Relaxed);
        let conn_count = active_connections.fetch_add(1, atomic::Ordering::Relaxed) + 1;
        info!("New connection (#{conn_id}) from {addr} ({conn_count} active)");
        let active_connections = Arc::clone(&active_connections);
        let config = Arc::clone(&config);
        let content = Arc::clone(&content);
        tokio::spawn(async move {
            let Ok(session_fut) = server::run_stream(
                config,
                stream,
                SshSession::new(conn_id, content),
            )
            .await else {
                error!("Error while setting up connection (#{conn_id}) from {addr}");
                return;
            };
            if let Err(e) = session_fut.await {
                error!("Error in connection (#{conn_id}) from {addr}: {e}");
            }
            let now_active = active_connections.fetch_sub(1, atomic::Ordering::Relaxed) - 1;
            info!("Connection (#{conn_id}) from {addr} closed ({now_active} active)");
        });
    }
}
