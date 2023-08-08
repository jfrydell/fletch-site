use std::{
    convert::Infallible,
    sync::{
        atomic::{self, AtomicUsize},
        Arc,
    },
    time::Duration,
};

use color_eyre::Result;
use russh::server::{self};
use russh_keys::key;
use tokio::{
    net::TcpListener,
    sync::{broadcast, mpsc, oneshot},
};
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
    let content = Arc::new(SshContent::new(&crate::CONTENT.read().unwrap())?);
    let mut config = server::Config::default();
    config.keys = vec![key::KeyPair::Ed25519(
        ed25519_dalek::Keypair::from_bytes(crate::CONFIG.ssh_key.to_bytes().as_ref()).unwrap(),
    )];
    let config = Arc::new(config);
    let listener = TcpListener::bind(("0.0.0.0", crate::CONFIG.ssh_port)).await?;

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
        // Clone vars for task
        let active_connections = Arc::clone(&active_connections);
        let config = Arc::clone(&config);
        let content = Arc::clone(&content);
        // Receiver for ChannelId to allow closing connection remotely
        let (channel_tx, channel_rx) = oneshot::channel();
        // Make channel to receive timeout resets
        let (timeout_reset, timeout_reset_rx) = mpsc::channel(1);
        tokio::spawn(async move {
            match server::run_stream(
                config,
                stream,
                SshSession::new(conn_id, content, channel_tx, timeout_reset),
            )
            .await
            {
                Ok(session_fut) => {
                    let _ = session_fut.await;
                }
                Err(_) => error!("Error while setting up connection (#{conn_id}) from {addr}"),
            };
            let now_active = active_connections.fetch_sub(1, atomic::Ordering::Relaxed) - 1;
            info!("Connection (#{conn_id}) from {addr} closed ({now_active} active)");
        });
        tokio::spawn(async move {
            // Get channel for closing connection
            let Ok(channel) = channel_rx.await else {
                error!("Error receiving channel for connection (#{conn_id}) from {addr} (presumably due to error in connection setup)");
                return;
            };
            // Wait for timeout and close connection
            if resetting_timeout(timeout_reset_rx, crate::CONFIG.ssh_timeout).await {
                info!("Connection (#{conn_id}) from {addr} timed out");
                if let Err(e) = channel.close().await {
                    error!("Error closing connection (#{conn_id}) from {addr}: {e}");
                }
            }
        });
    }
}

/// Helper function that times out (returning `true`) if no message is received within a certain duration. If the sender closes, the function returns `false`.
async fn resetting_timeout(mut reset_signal: mpsc::Receiver<()>, timeout: Duration) -> bool {
    loop {
        tokio::select! {
            _ = tokio::time::sleep(timeout) => {
                return true;
            }
            x = reset_signal.recv() => {
                if x.is_none() {
                    return false;
                }
                continue;
            }
        }
    }
}
