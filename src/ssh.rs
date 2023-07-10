use std::{
    borrow::Cow,
    collections::BTreeMap,
    sync::{atomic::AtomicUsize, Arc},
};

use anyhow::Result;
use async_trait::async_trait;
use russh::{
    server::{self, Msg, Session},
    Channel, ChannelId, CryptoVec, Disconnect,
};
use russh_keys::key;

use crate::project::Project;

/// Convert a project into a descriptive text file.
fn project_to_about(project: &Project) -> File {
    let big_contents = serde_json::to_string_pretty(project)
        .unwrap()
        .replace('\n', "\r\n");
    let contents = format!(
        "# {}\r\n\r\n{}\r\n\r\n{}",
        project.name, project.description, big_contents
    );
    File::new(project.name.clone(), contents)
}

/// The rendered content for the SSH server.
#[derive(Debug)]
pub struct SshContent {
    /// The directories of the virtual filesystem, with the root first.
    directories: Vec<Directory>,
}
impl SshContent {
    /// Render the SSH content from the given content.
    pub fn new(content: &crate::Content) -> Self {
        // Get an empty content to start
        let mut result = Self {
            directories: vec![Directory {
                path: "/".to_string(),
                ..Default::default()
            }],
        };

        // Add projects directory
        let projects_i = result.add_child(0, "projects".to_string());
        for project in content {
            let project_i = result.add_child(projects_i, project.url.clone());
            result.add_file(
                project_i,
                "about.txt".to_string(),
                project_to_about(project),
            );
        }

        result
    }
    /// Gets the directory at the given index.
    pub fn get(&self, i: usize) -> &Directory {
        &self.directories[i]
    }
    /// Gets the directory at the given path.
    pub fn dir_at(&self, path: &str) -> Option<&Directory> {
        let mut dir = &self.directories[0];
        for part in path.split('/') {
            if part.is_empty() || part == "." {
                continue;
            }
            if part == ".." {
                if let Some(parent) = dir.parent {
                    dir = &self.directories[parent];
                }
                continue;
            }
            dir = match dir.directories.get(part) {
                Some(i) => &self.directories[*i],
                None => return None,
            };
        }
        Some(dir)
    }
    /// Add a child directory to a `Directory` specified by index, returning the index of the child.
    fn add_child(&mut self, parent_i: usize, child_name: String) -> usize {
        let child_i = self.directories.len();
        let parent = &mut self.directories[parent_i];
        let child = Directory {
            path: {
                let mut path = parent.path.clone();
                if parent_i != 0 {
                    path.push('/');
                }
                path.push_str(&child_name);
                path
            },
            parent: Some(parent_i),
            ..Default::default()
        };
        parent.directories.insert(child_name, child_i);
        self.directories.push(child);
        child_i
    }
    /// Add a file to a `Directory` specified by index.
    fn add_file(&mut self, dir_i: usize, filename: String, contents: File) {
        let dir = &mut self.directories[dir_i];
        dir.files.insert(filename, contents);
    }
}

/// A directory in the virtual filesystem, containing a list of files and other directories.
#[derive(Debug, Default)]
pub struct Directory {
    /// The full path to this directory, always ending in a `/`.
    pub path: String,
    /// The parent of this directory (`None` if root).
    pub parent: Option<usize>,
    /// Subdirectories of this directory, indexed by name.
    pub directories: BTreeMap<String, usize>,
    /// Files in this directory, indexed by name.
    pub files: BTreeMap<String, File>,
}

/// A file in the virtual filesystem, containing an array of lines.
#[derive(Debug, Default)]
pub struct File {
    /// The name of the file.
    pub name: String,
    /// The raw contents of the file as a `String`.
    pub contents: String,
    /// The contents of the file, as an array of lines.
    pub lines: Vec<String>,
}
impl File {
    pub fn new(name: String, contents: String) -> Self {
        let lines: Vec<String> = contents.split("\r\n").map(|s| s.to_string()).collect();
        Self {
            name,
            contents,
            lines,
        }
    }
    pub fn raw_contents(&self) -> &[u8] {
        self.contents.as_bytes()
    }
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

pub async fn main(content: Arc<SshContent>) {
    let mut config = server::Config::default();
    config.keys = vec![key::KeyPair::generate_ed25519().unwrap()];
    let server = Server::new(content);
    println!("Starting SSH Server...");
    server::run(Arc::new(config), ("0.0.0.0", 2222), server)
        .await
        .expect("Running SSH server failed");
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
        println!("New client from {:?} assigned id {}", addr, id);
        SshSession::new(id, Arc::clone(&self.content))
    }
}

pub struct SshSession {
    id: usize,
    channel: Option<Channel<Msg>>,
    shell: Shell,
    content: Arc<SshContent>,
    current_dir: usize,
    term_size: (u32, u32),
    running_app: Option<Box<dyn RunningApp>>,
}
impl SshSession {
    fn new(id: usize, content: Arc<SshContent>) -> Self {
        Self {
            id,
            channel: None,
            shell: Shell::default(),
            content,
            current_dir: 0,
            term_size: (80, 24), // Just a guess, will be updated on connect anyway (TODO: make Option to do this right)
            running_app: None,
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

    async fn pty_request(
        mut self,
        _channel: ChannelId,
        _term: &str,
        col_width: u32,
        row_height: u32,
        _pix_width: u32,
        _pix_height: u32,
        _modes: &[(russh::Pty, u32)],
        session: Session,
    ) -> Result<(Self, Session), Self::Error> {
        println!(
            "got pty request (see russh/server/mod.rs: 497 for default impl, not sure if needed)"
        );
        self.term_size = (col_width, row_height);
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
        // println!("Client {} sent data: {:?}", self.id, data);

        // Process data
        let mut response = vec![];
        for i in data {
            match self.running_app {
                None => {
                    let (r, command) = self.shell.process(*i);
                    response.extend(r);
                    if let Some(command) = command {
                        println!("Client {} ran command: {:?}", self.id, command);
                        let command_name = command.split(' ').next().unwrap_or("");
                        match command_name {
                            "exit" | "logout" => {
                                session.disconnect(Disconnect::ByApplication, "Goodbye!", "");
                                return Ok((self, session));
                            }
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
                            "cat" => {
                                let file = command.split(' ').nth(1).unwrap_or("");
                                let current_dir = self.content.get(self.current_dir);
                                if let Some(content) = current_dir.files.get(file) {
                                    response.extend(content.raw_contents());
                                } else {
                                    response.extend(
                                        format!("\"{}\": no such file\r\n", file).as_bytes(),
                                    );
                                }
                            }
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
                            response.extend(self.shell.prompt());
                        }
                    }
                }
                Some(ref mut app) => {
                    if *i == 3 {
                        // CTRL-C, exit, clear screen, and reprompt
                        response.append(
                            &mut TerminalUtils::new().clear().move_cursor(0, 0).into_data(),
                        );
                        response.extend(self.shell.prompt());
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

/// A trait providing functionality for a running app (state machine), including the ability
/// to receive a byte of data and startup functionality.
trait RunningApp: Send {
    /// Starts the app, returning the initial state along with some initial reponse data
    /// (basically a starting render) on sucess. On failure, returns a response to send
    /// as if as a normal command (e.g. "file not found").
    fn startup(
        session: &SshSession,
        command: String,
    ) -> Result<(Box<dyn RunningApp>, Vec<u8>), Vec<u8>>
    where
        Self: Sized;
    /// Processes one byte of data from the user input, returning the response.
    fn data(&mut self, data: u8) -> Vec<u8>;
    /// Processes a resize request from the client, returning the response.
    fn resize(&mut self, width: u32, height: u32) -> Vec<u8>;
}

/// The state of a running instance of vim.
struct Vim<'a> {
    /// The content of the ssh server, kept to ensure that `self.file` stays alive.
    _ssh_content: Arc<SshContent>,
    /// Current cursor position (x,y), where (0,0) is the top left of the file.
    cursor_pos: (usize, usize),
    /// Current scroll position (line, subline), giving the line at the top of the screen and
    /// (if we've scrolled horizontally through a wrapping line) how many screen-widths of the
    /// line we've scrolled past already.
    scroll_pos: (usize, usize),
    /// The current size of the terminal, in characters.
    term_size: (u16, u16),
    /// The available height for the file (not including bottom text, usually term_size.1 - 1).
    available_height: usize,
    /// The file we are currently viewing.
    file: &'a File,
}
impl<'a> Vim<'a> {
    /// Helper method to clear and rerender the file, returning the necessary response to do so.
    ///
    /// Assumes that `cursor_pos` is onscreen for current `scroll_pos`.
    fn render(&self) -> Vec<u8> {
        // Clear the screen and move the cursor
        let mut response = TerminalUtils::new().clear().move_cursor(0, 0).into_data();

        // Output the file's contents, beginning at the scrolled location.
        // `lines` iterates through the file.
        // `current_line` holds the line of the file we're processing now.
        // `current_line_char` specifies where in the file's line this screen's line starts.
        let mut lines = self.file.lines.iter().skip(self.scroll_pos.0);
        let mut current_line = lines.next();
        let mut current_line_start = self.scroll_pos.1 * self.term_size.0 as usize;
        for y in 0..self.available_height {
            response.append(&mut TerminalUtils::new().move_cursor(0, y as u16).into_data());
            match current_line {
                None => {
                    // The file is over, print placeholder
                    response.push(b'~');
                }
                Some(line) if line.len() >= current_line_start + self.term_size.0 as usize => {
                    // Our current line will wrap, just print what we can and update `current_line_start`
                    response.extend(
                        line[current_line_start..current_line_start + self.term_size.0 as usize]
                            .as_bytes(),
                    );
                    current_line_start += self.term_size.0 as usize;
                }
                Some(line) => {
                    // Our current line will fit on this line, so print the rest of it and step forward
                    response.extend(line[current_line_start..].as_bytes());
                    current_line = lines.next();
                    current_line_start = 0;
                }
            }
        }
        response.extend(b"\r\n: Ctrl-H for help");

        // Reset the cursor, first finding screen coordinates. We assume that the current scroll is valid,
        // so we cast using `as`. If the coordinates are out of bounds, this is a bug / unconsidered edge case
        // where there are no valid coordinates, but `as` won't crash, just behave strangely.
        let (screen_x, screen_y) = self.get_cursor_screen();
        response.append(
            &mut TerminalUtils::new()
                .move_cursor(screen_x as u16, screen_y as u16)
                .into_data(),
        );

        response
    }

    /// Moves the cursor to the position in `cursor_pos`, scrolling and rerendering if necessary.
    fn update_cursor(&mut self) -> Vec<u8> {
        // Adjust to screen coordinates, and rescroll if they don't fit
        let mut must_rerender = false;
        let (screen_x, screen_y) = loop {
            let (screen_x, screen_y) = self.get_cursor_screen();
            if screen_y < 0 {
                // Must scroll up, first by subline then by line
                if self.scroll_pos.1 > 0 {
                    self.scroll_pos.1 -= 1;
                } else {
                    self.scroll_pos.0 -= 1;
                }
            } else if screen_y >= self.available_height as isize {
                // Must scroll down, by subline if possible (requires enough room in line)
                if (self.scroll_pos.1 + 1) * (self.term_size.0 as usize)
                    < self.file.lines[self.scroll_pos.0].len()
                {
                    self.scroll_pos.1 += 1;
                } else {
                    self.scroll_pos.1 = 0;
                    self.scroll_pos.0 += 1;
                }
            } else {
                break (screen_x as u16, screen_y as u16);
            }
            must_rerender = true;
        };

        // Render the new cursor, doing a full rerender if we scrolled.
        if must_rerender {
            self.render()
        } else {
            TerminalUtils::new()
                .move_cursor(screen_x, screen_y)
                .into_data()
        }
    }

    /// Helper to get the screen position of the cursor from the current `cursor_pos`, `scroll_pos`, and `term_size`.
    /// If this returns an out-of-bounds point, scrolling should be adjusted.
    fn get_cursor_screen(&self) -> (isize, isize) {
        // If cursor is behind first line of screen, or on it but left of scroll_pos, are above screen, so return (0, -1)
        if self.cursor_pos.1 < self.scroll_pos.0
            || self.cursor_pos.1 == self.scroll_pos.0
                && self.cursor_pos.0 < self.scroll_pos.1 * self.term_size.0 as usize
        {
            return (0, -1);
        }
        // Find the first line onscreen, and count how many lines until we get to the current line, including wrapping.
        // Start at `-self.scroll_pos.1` because first line may start above screen.
        let mut screen_y = -(self.scroll_pos.1 as isize);
        for line in self
            .file
            .lines
            .iter()
            .skip(self.scroll_pos.0)
            .take(self.cursor_pos.1 - self.scroll_pos.0)
        {
            screen_y += line.len() as isize / self.term_size.0 as isize + 1;
        }
        // Find the effective x position, snapping back to the end of short lines
        let effective_x =
            self.cursor_pos
                .0
                .min(self.file.lines[self.cursor_pos.1].len().max(1) - 1) as isize;
        // If effective x position is off screen, we will wrap, so adjust y and reduce x accordingly
        screen_y += effective_x / self.term_size.0 as isize;
        (effective_x % self.term_size.0 as isize, screen_y)
    }
}
impl<'a> RunningApp for Vim<'a> {
    fn startup(
        session: &SshSession,
        command: String,
    ) -> Result<(Box<(dyn RunningApp + 'static)>, Vec<u8>), Vec<u8>> {
        let content = session.content.clone();
        let full_path = command
            .split(' ')
            .nth(1)
            .ok_or_else(|| Vec::from(b"vi: usage: vi <filename>\r\n" as &[u8]))?;
        let (path, filename) = match full_path.rsplit_once('/') {
            Some((directory, filename)) => {
                if directory.starts_with('/') || directory.is_empty() {
                    // Absolute path, no need for current path addition
                    (Cow::Borrowed(directory), filename)
                } else {
                    let mut d = session.content.directories[session.current_dir]
                        .path
                        .clone();
                    d.push_str(directory);
                    (Cow::Owned(d), filename)
                }
            }
            None => (
                Cow::Borrowed(
                    session.content.directories[session.current_dir]
                        .path
                        .as_str(),
                ),
                full_path,
            ),
        };
        let file = content
            .dir_at(&path)
            .and_then(|d| d.files.get(filename))
            .ok_or_else(|| format!("vi: cannot open \"{}\": No such file\r\n", full_path))?;
        let vim = Vim {
            _ssh_content: Arc::clone(&content),
            cursor_pos: (0, 0),
            scroll_pos: (0, 0),
            term_size: (
                session.term_size.0.try_into().unwrap(),
                session.term_size.1.try_into().unwrap(),
            ),
            available_height: session.term_size.1 as usize - 1,
            // SAFETY: `file` references `content`, which is guarenteed to live as long as this `Vim` object due to the `_ssh_content` reference
            file: unsafe { &*(file as *const File) },
        };
        let response = vim.render();
        Ok((Box::new(vim), response))
    }
    fn data(&mut self, data: u8) -> Vec<u8> {
        match data {
            b'h'..=b'l' => {
                // Cursor movement
                let delta = match data {
                    b'h' => (-1, 0),
                    b'j' => (0, 1),
                    b'k' => (0, -1),
                    b'l' => (1, 0),
                    _ => unreachable!(),
                };
                // Get the new coordinates, clamped to the file's range.
                let new_y = (self.cursor_pos.1 as isize + delta.1 as isize)
                    .clamp(0, self.file.lines.len() as isize - 1)
                    as usize;
                let new_x = (self.cursor_pos.0 as isize + delta.0 as isize).max(0) as usize;
                self.cursor_pos = (new_x, new_y);
                // Update the cursor
                self.update_cursor()
            }
            b'$' => {
                // Move to end of line by setting cursor x to high value (not too high to avoid isize overflow)
                self.cursor_pos.0 = usize::MAX / 4;
                self.update_cursor()
            }
            _ => {
                println!("data '{data:?}' not implemented for vim");
                vec![]
            }
        }
    }
    fn resize(&mut self, width: u32, height: u32) -> Vec<u8> {
        self.term_size = (width as u16, height as u16);
        self.available_height = height as usize - 1;

        // Update scroll position to maintain wrapping invariant (start at beginning of line)
        let width = width as usize;
        self.scroll_pos.0 = (self.cursor_pos.0 / width) * width;
        self.render()
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
                // Backspace, remove character from buffer and overwrite as necessary
                if self.cursor > 0 {
                    line.remove(self.cursor - 1);
                    self.cursor -= 1;
                    if self.cursor == line.len() {
                        // At end of line, so go back, overwrite with space, go back again
                        (vec![8, 32, 8], None)
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

/// Some utilities for fancy terminal output.
///
/// ## Example
/// ```
/// let buffer: Vec<u8> = TerminalUtils::new(80, 24).place(40, 12).into_data();
/// ```
#[allow(unused)]
#[derive(Clone)]
struct TerminalUtils {
    pos: Option<(u16, u16)>,
    data: Vec<u8>,
}

#[allow(unused)]
impl TerminalUtils {
    /// Creates a new terminal utility for the given width and height.
    fn new() -> Self {
        Self {
            pos: None,
            data: vec![],
        }
    }

    /// Places a character `c` at a location (x,y).
    fn place(mut self, x: u16, y: u16, c: u8) -> Self {
        // Move cursor to new position if necessary, and update it
        if self.pos != Some((x, y)) {
            self = self.move_cursor(x, y);
        }
        self.pos = Some((x + 1, y));

        // Write symbol to queue of data to be sent
        self.data.push(c);
        self
    }
    /// Hides the cursor.
    fn hide_cursor(mut self) -> Self {
        self.data.extend(b"\x1b[?25l");
        self
    }
    /// Shows the cursor.
    fn show_cursor(mut self) -> Self {
        self.data.extend(b"\x1b[?25h");
        self
    }
    /// Moves the cursor.
    fn move_cursor(mut self, x: u16, y: u16) -> Self {
        self.data
            .extend(format!("\x1b[{};{}H", y + 1, x + 1).as_bytes());
        self
    }
    /// Clears the screen (doesn't move cursor).
    fn clear(mut self) -> Self {
        self.data.extend(b"\x1b[2J");
        self
    }

    /// Gets the data for all the operations done.
    fn into_data(self) -> Vec<u8> {
        self.data
    }
}
