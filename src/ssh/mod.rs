use std::{
    convert::Infallible,
    sync::{atomic::AtomicUsize, Arc},
};

use color_eyre::Result;
use russh::server::{self};
use russh_keys::key;
use tokio::sync::broadcast;
use tracing::info;

use crate::ssh::content::SshContent;

use self::session::SshSession;

mod apps;
mod content;
mod session;
mod terminal;

pub async fn main(_rx: broadcast::Receiver<()>) -> Result<Infallible> {
    // TODO: add live-reload when we get message from _rx
    let content = Arc::new(SshContent::new(&crate::CONTENT.read().unwrap()));
    let mut config = server::Config::default();
    config.keys = vec![key::KeyPair::generate_ed25519().unwrap()];
    let server = Server::new(content);

    info!("Starting SSH Server...");
    server::run(Arc::new(config), ("0.0.0.0", 22), server)
        .await
        .expect("Running SSH server failed");
    #[allow(unreachable_code)]
    Ok(unreachable!("SSH server shouldn't exit without an error"))
}

#[derive(Debug)]
pub struct Server {
    id: AtomicUsize,
    content: Arc<SshContent>,
}
impl Server {
    fn new(content: Arc<SshContent>) -> Self {
        Self {
            id: AtomicUsize::new(0),
            content,
        }
    }
}
impl server::Server for Server {
    type Handler = SshSession;
    fn new_client(&mut self, addr: Option<std::net::SocketAddr>) -> Self::Handler {
        let id = self.id.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        info!("New client from {:?} assigned id {}", addr, id);
        SshSession::new(id, Arc::clone(&self.content))
    }
}
