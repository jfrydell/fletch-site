//! Implements the POP3 protocol, to browse the site as if it's a mail server.

use std::{convert::Infallible, sync::Arc};

use color_eyre::Result;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::{TcpListener, TcpStream},
    sync::broadcast,
};
use tracing::debug;

use crate::Content;

/// Runs the POP server, updating the content on `update_rx`.
pub async fn main(_update_rx: broadcast::Receiver<()>) -> Result<Infallible> {
    let content = Pop3Content::from(&*crate::CONTENT.read().unwrap());
    let content = Arc::new(content);

    let tcp_listener = TcpListener::bind(("0.0.0.0", crate::CONFIG.pop3_port)).await?;
    loop {
        let new_connection = tcp_listener.accept().await?;
        debug!("New POP3 connection from {}", new_connection.1);
        tokio::spawn(handle_connection(new_connection.0, Arc::clone(&content)));
    }
}

/// Handles one POP3 connection.
async fn handle_connection(mut connection: TcpStream, content: Arc<Pop3Content>) -> Result<()> {
    // Split connection to get `BufReader`
    let (reader, mut writer) = connection.split();
    let mut reader = BufReader::new(reader).lines();

    // Macro to get command from stream
    macro_rules! get_command {
        () => {
            match reader.next_line().await? {
                Some(line) => Pop3Command::new(&line),
                None => return Ok(()),
            }
        };
    }

    // Send greeting and wait for authentication
    writer.write(b"+OK POP3 server ready\r\n").await?;
    let mut got_user = false;
    let mut got_pass = false;
    while !got_user || !got_pass {
        let command = get_command!();
        match command {
            Pop3Command::User(_) => {
                writer.write(b"+OK\r\n").await?;
                got_user = true;
            }
            Pop3Command::Pass(_) => {
                writer.write(b"+OK\r\n").await?;
                got_pass = true;
            }
            Pop3Command::Quit => {
                writer.write(b"+OK\r\n").await?;
                return Ok(());
            }
            _ => {
                writer.write(b"-ERR\r\n").await?;
            }
        }
    }

    // Transaction state (handle normal commands)
    loop {
        match get_command!() {
            Pop3Command::Quit => {
                writer.write(b"+OK\r\n").await?;
                return Ok(());
            }
            Pop3Command::Stat => {
                writer
                    .write(
                        format!("+OK {} {}\r\n", content.messages.len(), content.total_size)
                            .as_bytes(),
                    )
                    .await?;
            }
            Pop3Command::List(Some(i)) => {
                if i >= content.messages.len() {
                    writer.write(b"-ERR\r\n").await?;
                } else {
                    let message = &content.messages[i - 1];
                    writer
                        .write(format!("+OK {} {}\r\n", i, message.size).as_bytes())
                        .await?;
                }
            }
            Pop3Command::List(None) => {
                writer.write(b"+OK\r\n").await?;
                for (i, message) in content.messages.iter().enumerate() {
                    writer
                        .write(format!("{} {}\r\n", i + 1, message.size).as_bytes())
                        .await?;
                }
                writer.write(b".\r\n").await?;
            }
            Pop3Command::Retr(i) => {
                if i >= content.messages.len() {
                    writer.write(b"-ERR\r\n").await?;
                } else {
                    let message = &content.messages[i - 1];
                    writer.write(b"+OK\r\n").await?;
                    for line in &message.lines {
                        writer.write(line.as_bytes()).await?;
                    }
                    writer.write(b".\r\n").await?;
                }
            }
            Pop3Command::Noop => {
                writer.write(b"+OK\r\n").await?;
            }
            Pop3Command::Rset => {
                writer.write(b"+OK\r\n").await?;
            }
            _ => {
                writer.write(b"-ERR\r\n").await?;
            }
        }
    }
}

/// All supported POP3 commands, able to be parsed from a string.
enum Pop3Command {
    User(String),
    Pass(String),
    Quit,
    Stat,
    List(Option<usize>),
    Retr(usize),
    Noop,
    Rset,
    /// An invalid or unsupported command.
    Invalid,
}
impl Pop3Command {
    fn new(line: &str) -> Self {
        let mut split = line.split_whitespace();
        match split.next() {
            Some("USER") => Pop3Command::User(split.next().unwrap_or("").to_string()),
            Some("PASS") => Pop3Command::Pass(split.next().unwrap_or("").to_string()),
            Some("QUIT") => Pop3Command::Quit,
            Some("STAT") => Pop3Command::Stat,
            Some("LIST") => Pop3Command::List(split.next().and_then(|s| s.parse().ok())),
            Some("RETR") => {
                Pop3Command::Retr(split.next().and_then(|s| s.parse().ok()).unwrap_or(0))
            }
            Some("NOOP") => Pop3Command::Noop,
            Some("RSET") => Pop3Command::Rset,
            _ => Pop3Command::Invalid,
        }
    }
}

/// The POP3 maildrop content, including messages for each page on the site.
struct Pop3Content {
    // A message (in POP3 multiline format) for each page on the site.
    //
    // The first element corresponds to POP3 message 1 (1-indexed).
    pub messages: Vec<Pop3Message>,
    pub total_size: usize,
}

/// A single message in the POP3 maildrop.
struct Pop3Message {
    /// The lines of the message, including the terminating `"\r\n"`. Any lines starting with `.` are byte-stuffed.
    pub lines: Vec<String>,
    pub size: usize,
}
impl From<&Content> for Pop3Content {
    fn from(content: &Content) -> Self {
        // Generate all pages of the site
        let mut pages = Vec::new();
        pages.push("From: Fletch\nTo: You!\nSubject: Welcome!\n\nHello! Welcome to my website, exposed via a POP3 mail server. All the pages should be listed here as emails, so feel free to browse around!".to_string());
        for project in content.projects.iter() {
            pages.push(format!(
                "From: Fletch\nTo: You!\nSubject: {}\n\n{}",
                project.name,
                project.to_string(),
            ));
        }

        // Convert pages to POP3 messages
        let mut messages = Vec::new();
        let mut total_size = 0;
        for page in pages {
            let mut lines = Vec::new();
            let mut message_size = 0;
            for line in page.lines() {
                // Byte-stuff lines starting with `.`
                let line = if line.starts_with('.') {
                    format!(".{}\r\n", line)
                } else {
                    format!("{}\r\n", line)
                };
                message_size += line.len();
                lines.push(line);
            }
            messages.push(Pop3Message {
                lines,
                size: message_size,
            });
            total_size += message_size;
        }
        Pop3Content {
            messages,
            total_size,
        }
    }
}
