use std::sync::Arc;

use async_trait::async_trait;
use color_eyre::Result;
use russh::{
    server::{self, Msg, Session},
    Channel, ChannelId, CryptoVec, Disconnect,
};

use russh_keys::key;
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, info};

use crate::ssh::{apps::Vim, content::WELCOME_MESSAGE};

use super::{
    apps::RunningApp,
    content::SshContent,
    terminal::{Shell, TerminalUtils},
};

pub struct SshSession {
    id: usize,
    shell: Shell,
    pub username: String,
    pub content: Arc<SshContent>,
    pub current_dir: usize,
    pub term_size: (u32, u32),
    pub running_app: Option<Box<dyn RunningApp>>,
    /// A channel to send the russh `ChannelId` back to the main loop, allowing it to close the connection remotely. After sending the channel, this is set to `None`.
    pub channel_tx: Option<oneshot::Sender<Channel<Msg>>>,
    /// A channel to refresh the timeout on this session.
    pub timeout_refresh: mpsc::Sender<()>,
}
impl SshSession {
    pub fn new(
        id: usize,
        content: Arc<SshContent>,
        channel_tx: oneshot::Sender<Channel<Msg>>,
        timeout_refresh: mpsc::Sender<()>,
    ) -> Self {
        Self {
            id,
            shell: Shell::default(),
            username: String::new(),
            content,
            current_dir: 0,
            term_size: (80, 24), // Just a guess, will be updated on connect anyway (TODO: make Option to do this right)
            running_app: None,
            timeout_refresh,
            channel_tx: Some(channel_tx),
        }
    }
    /// Handle auth, accepting everyone and setting the username.
    pub async fn auth(
        mut self,
        user: &str,
    ) -> Result<(Self, server::Auth), <Self as server::Handler>::Error> {
        info!("Client {} authenticated as {}", self.id, user);
        self.username = user.to_string();
        Ok((self, server::Auth::Accept))
    }
    /// Get the current prompt.
    pub fn prompt(&self) -> Vec<u8> {
        let mut prompt = self.username.as_bytes().to_vec();
        prompt.push(b'@');
        prompt.extend(crate::CONFIG.domain.as_bytes());
        prompt.push(b':');
        prompt.extend(self.content.get(self.current_dir).path.as_bytes());
        prompt.extend(b"> ");
        prompt
    }
}

#[async_trait]
impl server::Handler for SshSession {
    type Error = color_eyre::Report;

    async fn channel_open_session(
        mut self,
        channel: Channel<Msg>,
        session: Session,
    ) -> Result<(Self, bool, Session), Self::Error> {
        // If we haven't opened a channel yet, send the channel ID back to the main loop and allow it. Otherwise, reject the channel (to keep only one going).
        // Not sure if this is at all a good idea, but it works for now (chosen because it's the only way I could find to close the connection from the main loop).
        if let Some(tx) = self.channel_tx.take() {
            tx.send(channel)
                .map_err(|_| color_eyre::eyre::eyre!("Failed to send channel ID to main loop"))?;
            Ok((self, true, session))
        } else {
            Ok((self, false, session))
        }
    }

    async fn auth_none(self, user: &str) -> Result<(Self, server::Auth), Self::Error> {
        self.auth(user).await
    }
    async fn auth_password(self, user: &str, _: &str) -> Result<(Self, server::Auth), Self::Error> {
        self.auth(user).await
    }
    async fn auth_publickey(
        self,
        user: &str,
        _: &key::PublicKey,
    ) -> Result<(Self, server::Auth), Self::Error> {
        self.auth(user).await
    }

    async fn pty_request(
        mut self,
        channel: ChannelId,
        _term: &str,
        col_width: u32,
        row_height: u32,
        _pix_width: u32,
        _pix_height: u32,
        _modes: &[(russh::Pty, u32)],
        mut session: Session,
    ) -> Result<(Self, Session), Self::Error> {
        debug!(
            "got pty request (see russh/server/mod.rs: 497 for default impl, not sure if needed)"
        );
        self.term_size = (col_width, row_height);
        session.data(channel, Vec::from(WELCOME_MESSAGE).into());
        session.data(channel, CryptoVec::from(self.prompt()));
        Ok((self, session))
    }

    async fn window_change_request(
        mut self,
        channel: ChannelId,
        col_width: u32,
        row_height: u32,
        _pix_width: u32,
        _pix_height: u32,
        mut session: Session,
    ) -> Result<(Self, Session), Self::Error> {
        self.term_size = (col_width, row_height);
        if let Some(ref mut running_app) = self.running_app {
            let resp = running_app.resize(col_width, row_height);
            session.data(channel, CryptoVec::from(resp));
        }
        Ok((self, session))
    }

    async fn data(
        mut self,
        channel: ChannelId,
        data: &[u8],
        mut session: Session,
    ) -> Result<(Self, Session), Self::Error> {
        self.timeout_refresh.send(()).await?;
        // println!("Client {} sent data: {:?}", self.id, data);

        // Process data
        let mut response = vec![];
        for i in data {
            match self.running_app {
                None => {
                    // No app running, so shell handles input
                    let (r, command) = self.shell.process(*i);
                    response.extend(r);
                    if let Some(command) = command {
                        info!("Client {} ran command: {:?}", self.id, command);
                        let command_name = command.split(' ').next().unwrap_or("");
                        match command_name {
                            "exit" | "logout" => {
                                session.disconnect(Disconnect::ByApplication, "Goodbye!", "");
                                return Ok((self, session));
                            }
                            "help" => response.extend(super::content::WELCOME_MESSAGE),
                            "ls" => {
                                let current_dir = self.content.get(self.current_dir);
                                for (name, _) in current_dir.directories.iter() {
                                    response.extend(format!("{}\r\n", name).as_bytes());
                                }
                                for (name, _) in current_dir.files.iter() {
                                    response.extend(format!("{}\r\n", name).as_bytes());
                                }
                            }
                            "cd" => {
                                let dir = command.split(' ').nth(1).unwrap_or("");
                                let current_dir = self.content.get(self.current_dir);
                                if dir == ".." {
                                    if let Some(id) = current_dir.parent {
                                        self.current_dir = id;
                                    }
                                } else if let Some(&id) = current_dir.directories.get(dir) {
                                    self.current_dir = id;
                                } else {
                                    response.extend(
                                        format!("\"{}\": no such directory\r\n", dir).as_bytes(),
                                    );
                                }
                            }
                            "cat" => match command.split(' ').nth(1) {
                                None => response.extend(b"cat: usage: cat <filename>\r\n"),
                                Some(path) => {
                                    match self.content.get_file(self.current_dir, path) {
                                        None => response.extend(
                                            format!(
                                                "cat: cannot open \"{}\": No such file\r\n",
                                                path
                                            )
                                            .as_bytes(),
                                        ),
                                        Some(file) => {
                                            response.extend(file.raw_contents());
                                        }
                                    };
                                }
                            },
                            "vi" => match Vim::startup(&self, command) {
                                Ok((running_app, mut startup_resp)) => {
                                    self.running_app = Some(running_app);
                                    response.append(&mut startup_resp);
                                }
                                Err(mut error_resp) => {
                                    response.append(&mut error_resp);
                                }
                            },
                            "" => {}
                            _ => {
                                response.extend(
                                    format!("{}: command not found\r\n", command).as_bytes(),
                                );
                            }
                        }
                        if self.running_app.is_none() {
                            // No app was started, so reprompt
                            response.extend(self.prompt());
                        }
                    }
                }
                Some(ref mut app) => {
                    if *i == 3 {
                        // CTRL-C, exit, clear screen and reprompt
                        response.append(
                            &mut TerminalUtils::new().clear().move_cursor(0, 0).into_data(),
                        );
                        response.extend(self.prompt());
                        self.running_app = None;
                    } else {
                        response.extend(app.data(*i));
                    }
                }
            }
        }

        // Send back to client
        let data = CryptoVec::from(response);
        session.data(channel, data);

        Ok((self, session))
    }
}
