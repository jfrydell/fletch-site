use std::{
    collections::BTreeMap,
    sync::{atomic::AtomicUsize, Arc, RwLock, Weak},
};

use anyhow::Result;
use async_trait::async_trait;
use russh::{
    server::{self, Msg, Session},
    Channel, ChannelId, CryptoVec, Disconnect,
};
use russh_keys::key;

/// The rendered content for the SSH server.
#[derive(Debug)]
pub struct SshContent {
    /// The root directory of the virtual filesystem
    pub root: Arc<Directory>,
}
impl SshContent {
    /// Render the SSH content from the given content.
    pub fn render(&mut self, _content: &crate::Content) {
        unimplemented!()
    }
}
impl Default for SshContent {
    fn default() -> Self {
        let root = Arc::new(Directory {
            path: "/".to_string(),
            parent: None,
            directories: BTreeMap::new(),
            files: BTreeMap::new(),
        });
        Self { root }
    }
}

/// A directory in the virtual filesystem, containing a list of files and other directories.
#[derive(Debug)]
pub struct Directory {
    /// The full path to this directory.
    pub path: String,
    /// The parent of this directory (`None` if root).
    pub parent: Option<Weak<Directory>>,
    /// Subdirectories of this directory, indexed by name.
    pub directories: BTreeMap<String, Arc<Directory>>,
    /// Files in this directory, indexed by name.
    pub files: BTreeMap<String, String>,
}

static WELCOME_MESSAGE: &[u8] = "=====================================\r
|========== FLETCH RYDELL ==========|\r
|========== *ssh edition* ==========|\r
|===================================|\r
|Welcome to the SSH version of my   |\r
|website! This is a work in progress|\r
|but I hope you enjoy it!           |\r
|===================================|\r
|To navigate, use the 'ls' and 'cd' |\r
|commands to see the available pages|\r
|and 'cat' or 'vi' to view them.    |\r
|Type 'exit' or 'logout' to leave.  |\r
=====================================\r
"
.as_bytes();

pub async fn main(content: Arc<RwLock<SshContent>>) {
    let mut config = server::Config::default();
    config.keys = vec![key::KeyPair::generate_ed25519().unwrap()];
    let server = Server::new(content);
    server::run(Arc::new(config), ("0.0.0.0", 2222), server)
        .await
        .expect("Running SSH server failed");
}

#[derive(Debug)]
pub struct Server {
    id: AtomicUsize,
    content: Arc<RwLock<SshContent>>,
}
impl Server {
    fn new(content: Arc<RwLock<SshContent>>) -> Self {
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
        println!("New client from {:?} assigned id {}", addr, id);
        SshSession::new(id, Arc::clone(&self.content))
    }
}

pub struct SshSession {
    id: usize,
    channel: Option<Channel<Msg>>,
    shell: Shell,
    content: Arc<RwLock<SshContent>>,
}
impl SshSession {
    fn new(id: usize, content: Arc<RwLock<SshContent>>) -> Self {
        Self {
            id,
            channel: None,
            shell: Shell::default(),
            content,
        }
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
        _id: ChannelId,
        _max_packet_size: u32,
        _window_size: u32,
        session: Session,
    ) -> Result<(Self, Session), Self::Error> {
        Ok((self, session))
    }

    async fn shell_request(
        self,
        channel: ChannelId,
        mut session: Session,
    ) -> Result<(Self, Session), Self::Error> {
        println!("Client {} requested shell", self.id);

        session.data(channel, Vec::from(WELCOME_MESSAGE).into());
        session.data(channel, CryptoVec::from(self.shell.prompt()));
        Ok((self, session))
    }

    async fn data(
        mut self,
        channel: ChannelId,
        data: &[u8],
        mut session: Session,
    ) -> Result<(Self, Session), Self::Error> {
        // println!("Client {} sent data: {:?}", self.id, data);

        // Process data
        let mut response = vec![];
        for i in data {
            let (r, command) = self.shell.process(*i);
            response.extend(r);
            if let Some(command) = command {
                println!("Client {} ran command: {:?}", self.id, command);
                match command.as_str() {
                    "exit" | "logout" => {
                        session.disconnect(Disconnect::ByApplication, "Goodbye!", "");
                        return Ok((self, session));
                    }
                    "ls" => {
                        response.extend(b"index.html\r\n");
                    }
                    _ => {
                        response.extend(format!("{}: command not found\r\n", command).as_bytes());
                    }
                }
                response.extend(self.shell.prompt());
            }
        }

        // Send back to client?
        let data = CryptoVec::from(response);
        session.data(channel, data);

        Ok((self, session))
    }
}

/// A virtual shell implementing line discipline, echoing, and backspace, receiving individual character inputs and passing output back to the client.
#[derive(Debug)]
struct Shell {
    /// The current cursor position
    cursor: usize,
    /// Command history
    history: Vec<String>,
    /// Current command history, as explored by up/down arrow. First element is the "current" (newest) line.
    ///
    /// This history is not saved between command runs, but is duplicated so the user can edit past commands without affecting the real history.
    current_history: Vec<String>,
    /// Index into the current_history specifying what we're editing now
    history_index: usize,
    /// Escape sequence buffer
    escape: Vec<u8>,
}
impl Default for Shell {
    fn default() -> Self {
        Self {
            cursor: 0,
            history: vec![],
            current_history: vec![String::new()],
            history_index: 0,
            escape: vec![],
        }
    }
}

impl Shell {
    /// Returns the shell prompt (done for initialization and upon switching back to the shell after a command)
    pub fn prompt(&self) -> Vec<u8> {
        vec![62, 32]
    }

    /// Processes a byte of input, returning a response to send back as well as optionally a command to run
    ///
    /// (Some logic taken from [https://github.com/offirgolan/Shell/blob/master/read-line.c])
    pub fn process(&mut self, data: u8) -> (Vec<u8>, Option<String>) {
        if !self.escape.is_empty() {
            return (self.process_escape(data), None);
        }
        let line = {
            self.get_line();
            self.current_history.get_mut(self.history_index).unwrap()
        };
        match data {
            13 | 10 => {
                // Newline, echo and run command
                let command = std::mem::take(line);
                self.history.push(command.clone());
                // Reset current history/command
                self.current_history = vec![String::new()];
                self.history_index = 0;
                self.cursor = 0;
                (vec![13, 10], Some(command))
            }
            8 | 127 => {
                // Backspace, remove character and send [backspace, delete, backspace] to overwrite
                if self.cursor > 0 {
                    line.remove(self.cursor - 1);
                    self.cursor -= 1;
                    if self.cursor == line.len() {
                        // At end of line, so go back, overwrite with space, go back again
                        (vec![8, 127, 8], None)
                    } else {
                        // Middle of line, so go back, overwrite with rest of line, go back to original location
                        let mut response = vec![8];
                        response.extend(line[self.cursor..].bytes());
                        response.push(32); // Overwrite last character (since new line has one fewer than old)
                        response.extend(std::iter::repeat(8).take(line.len() - self.cursor + 1));
                        (response, None)
                    }
                } else {
                    (vec![], None)
                }
            }
            3 => {
                // CTRL-C, clear line and reset without running command
                // Reset current history/command
                self.current_history = vec![String::new()];
                self.history_index = 0;
                self.cursor = 0;
                // Send newline and prompt withour running command
                let mut response = vec![13, 10];
                response.extend(self.prompt());
                (response, None)
            }
            4 => {
                // CTRL-D, close session
                (vec![], Some("exit".to_string()))
            }
            1 => {
                // CTRL-A, move cursor to start of line
                let response = vec![8; self.cursor];
                self.cursor = 0;
                (response, None)
            }
            5 => {
                // CTRL-E, move cursor to end of line
                let response = vec![8; line.len() - self.cursor];
                self.cursor = line.len();
                (response, None)
            }
            27 => {
                // Escape sequence, wait for next two bytes
                self.escape = vec![27];
                (vec![], None)
            }
            32.. => {
                // Normal character, insert and echo
                line.insert(self.cursor, data as char);
                self.cursor += 1;
                if self.cursor < line.len() {
                    // Inserted in the middle, send [inserted, rest of line, move cursor back]
                    let mut response = vec![];
                    response.push(data);
                    response.extend(line[self.cursor..].bytes());
                    response.extend(vec![8; line.len() - self.cursor]);
                    (response, None)
                } else {
                    // Inserted at the end, send [inserted]
                    (vec![data], None)
                }
            }
            _ => (vec![], None),
        }
    }

    /// Processes a byte of data while in the middle of an escape sequence
    fn process_escape(&mut self, data: u8) -> Vec<u8> {
        self.escape.push(data);
        if self.escape.len() == 3 {
            // Escape sequence complete
            // Get current line, updating histories if necessary
            let line = {
                self.get_line();
                self.current_history.get(self.history_index).unwrap()
            };
            let escape = std::mem::take(&mut self.escape);
            match escape.as_slice() {
                [27, 91, 68] => {
                    // Left arrow, move cursor back
                    if self.cursor > 0 {
                        self.cursor -= 1;
                        vec![27, 91, 68]
                    } else {
                        vec![]
                    }
                }
                [27, 91, 67] => {
                    // Right arrow, move cursor forward
                    if self.cursor < line.len() {
                        self.cursor += 1;
                        vec![27, 91, 67]
                    } else {
                        vec![]
                    }
                }
                [27, 91, 65] | [27, 91, 66] => {
                    // Up or down arrow, move in history
                    if escape[2] == 65 {
                        // Up arrow
                        self.history_index += 1;
                    } else {
                        // Down arrow, return if already at current
                        if self.history_index == 0 {
                            return vec![];
                        }
                        self.history_index -= 1;
                    }
                    // Clear current line
                    let mut response = vec![8; self.cursor];
                    response.extend(std::iter::repeat(32).take(line.len()));
                    response.extend(std::iter::repeat(8).take(line.len()));
                    // Get new line
                    let line = {
                        self.get_line();
                        self.current_history.get(self.history_index).unwrap()
                    };
                    // Write new line and update cursor
                    self.cursor = line.len();
                    response.extend(line.bytes());
                    response
                }
                _ => vec![],
            }
        } else {
            vec![]
        }
    }

    /// Get the current line of input, updating the current history and clamping the history index if necessary.
    /// Rather then returning a reference, we just update `self.current_history` and `self.history_index` to avoid borrowing issues.
    fn get_line(&mut self) {
        // Clamp history index so we don't go too far back
        if self.history_index > self.history.len() {
            self.history_index = self.history.len();
        }
        // Extend current history to reach history index
        while self.current_history.len() <= self.history_index {
            self.current_history
                .push(self.history[self.history.len() - self.current_history.len()].clone())
        }
    }
}
