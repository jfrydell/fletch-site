use std::{
    sync::{atomic::AtomicUsize, Arc},
    vec,
};

use anyhow::Result;
use async_trait::async_trait;
use russh::{
    server::{self, Msg, Session},
    Channel, ChannelId, CryptoVec, Disconnect, Pty,
};
use russh_keys::key;

pub async fn main() {
    let mut config = server::Config::default();
    config.keys = vec![key::KeyPair::generate_ed25519().unwrap()];
    let server = Server::default();
    server::run(Arc::new(config), ("0.0.0.0", 2222), server)
        .await
        .expect("Running SSH server failed");
}

#[derive(Debug, Default)]
pub struct Server {
    id: AtomicUsize,
}
impl server::Server for Server {
    type Handler = SshSession;
    fn new_client(&mut self, addr: Option<std::net::SocketAddr>) -> Self::Handler {
        let id = self.id.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        println!("New client from {:?} assigned id {}", addr, id);
        SshSession::new(id)
    }
}

#[derive(Default)]
pub struct SshSession {
    id: usize,
    channel: Option<Channel<Msg>>,
    command: String,
}
impl SshSession {
    fn new(id: usize) -> Self {
        Self {
            id,
            ..Default::default()
        }
    }

    fn run_command(&mut self) {
        println!("Client {} ran command: {:?}", self.id, self.command);
        self.command.clear();
    }
}

#[async_trait]
impl server::Handler for SshSession {
    type Error = anyhow::Error;

    async fn channel_open_session(
        mut self,
        channel: Channel<Msg>,
        session: Session,
    ) -> Result<(Self, bool, Session), Self::Error> {
        match self.channel.as_mut() {
            Some(c) => {
                println!("Client {} already has a channel open ({:?})", self.id, c);
                Ok((self, false, session))
            }
            None => {
                self.channel = Some(channel);
                Ok((self, true, session))
            }
        }
    }

    async fn auth_publickey(
        self,
        _: &str,
        _: &key::PublicKey,
    ) -> Result<(Self, server::Auth), Self::Error> {
        Ok((self, server::Auth::Accept))
    }

    async fn channel_open_confirmation(
        self,
        id: ChannelId,
        _max_packet_size: u32,
        _window_size: u32,
        mut session: Session,
    ) -> Result<(Self, Session), Self::Error> {
        session.data(id, CryptoVec::from(vec![62, 32]));
        Ok((self, session))
    }

    async fn data(
        mut self,
        channel: ChannelId,
        data: &[u8],
        mut session: Session,
    ) -> Result<(Self, Session), Self::Error> {
        if data.contains(&3) {
            println!("Client {} sent Ctrl-C", self.id);
            session.disconnect(Disconnect::ByApplication, "Ctrl-C received", "");
            return Ok((self, session));
        }
        // println!("Client {} sent data: {:?}", self.id, data,);

        // Process data
        let mut response = vec![];
        for i in data {
            match i {
                13 => {
                    // Newline sent
                    response.extend([13, 10]);
                    self.run_command();
                    response.extend([62, 32]);
                }
                127 => {
                    // Backspace sent
                    response.extend([8, 127, 8]);
                    self.command.pop();
                }
                _ => {
                    response.push(*i);
                    self.command.push(*i as char);
                }
            }
        }

        // Send back to client?
        let data = CryptoVec::from(response);
        session.data(channel, data);

        Ok((self, session))
    }
}
