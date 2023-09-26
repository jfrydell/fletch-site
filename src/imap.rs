//! Implements the IMAP protocol, to browse the site as if it's a mail server.

use std::{convert::Infallible, sync::Arc};

use color_eyre::Result;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::{TcpListener, TcpStream},
    sync::broadcast,
};
use tracing::debug;

use crate::Content;

/// Runs the IMAP server, updating the content on `update_rx`.
pub async fn main(_update_rx: broadcast::Receiver<()>) -> Result<Infallible> {
    let content = ImapContent::from(&*crate::CONTENT.read().unwrap());
    let content = Arc::new(content);

    let tcp_listener = TcpListener::bind(("0.0.0.0", crate::CONFIG.imap_port)).await?;
    loop {
        let new_connection = tcp_listener.accept().await?;
        debug!("New IMAP connection from {}", new_connection.1);
        tokio::spawn(handle_connection(new_connection.0, Arc::clone(&content)));
    }
}

/// Handles one IMAP connection.
async fn handle_connection(mut connection: TcpStream, content: Arc<ImapContent>) -> Result<()> {
    // Split connection to get `BufReader`
    let (reader, mut writer) = connection.split();
    let mut reader = BufReader::new(reader).lines();

    // Macro to send a response with optional tag
    macro_rules! send {
        ($tag:expr, $response:expr) => {
            writer
                .write(format!("{} {}\r\n", $tag, $response).as_bytes())
                .await?;
        };
        ($tag:expr) => {
            writer.write(format!("* {}\r\n", $tag).as_bytes()).await?;
        };
    }

    // Send greeting and wait for authentication
    send!("OK IMAP2 Service Ready");
    loop {
        break;
    }

    // Transaction state (handle normal commands)
    loop {
        let Some(command) = reader
            .next_line()
            .await?
            .and_then(|line| ImapCommand::new(&line))
        else {
            return Ok(());
        };
        match command.command {
            ImapCommandType::Noop => {
                send!(command.tag, "OK");
            }
            ImapCommandType::Login(_, _) => {
                send!(command.tag, "OK LOGIN completed");
            }
            ImapCommandType::Logout => {
                send!("BYE IMAP2 server terminating connection");
                send!(command.tag, "OK LOGOUT completed");
                break;
            }
            ImapCommandType::Select(mailbox) => {
                send!(format!("{} EXISTS", content.messages.len()));
                send!(format!("FLAGS ()"));
                send!(format!("{} RECENT", content.messages.len()));
                send!(command.tag, "OK [READ-WRITE] SELECT completed");
            }
            ImapCommandType::Check => {
                send!(format!("{} EXISTS", content.messages.len()));
                send!(command.tag, "OK CHECK completed");
            }
            ImapCommandType::Expunge => todo!(),
            ImapCommandType::Copy(_, _) => todo!(),
            ImapCommandType::Search(_) => todo!(),
        }
    }
    Ok(())
}

/// All supported IMAP commands, able to be parsed from a string.
struct ImapCommand {
    pub tag: String,
    pub command: ImapCommandType,
}
impl ImapCommand {
    fn new(line: &str) -> Option<Self> {
        let mut split = line.split_whitespace();
        let tag = split.next().unwrap().to_string();
        let command = match split.next().unwrap().to_ascii_lowercase().as_str() {
            "noop" => ImapCommandType::Noop,
            "login" => ImapCommandType::Login(
                split.next().unwrap().to_string(),
                split.next().unwrap().to_string(),
            ),
            "logout" => ImapCommandType::Logout,
            "select" => ImapCommandType::Select(split.next().unwrap().to_string()),
            "check" => ImapCommandType::Check,
            "expunge" => ImapCommandType::Expunge,
            "copy" => ImapCommandType::Copy(
                split.next().unwrap().to_string(),
                split.next().unwrap().to_string(),
            ),
            "search" => ImapCommandType::Search(split.next().unwrap().to_string()),
            _ => return None,
        };
        Some(ImapCommand { tag, command })
    }
}
pub enum ImapCommandType {
    Noop,
    Login(String, String),
    Logout,
    Select(String),
    Check,
    Expunge,
    Copy(String, String),
    Search(String),
}

/// The IMAP maildrop content, including messages for each page on the site.
struct ImapContent {
    // A message (in IMAP multiline format) for each page on the site.
    //
    // The first element corresponds to IMAP message 1 (1-indexed).
    pub messages: Vec<ImapMessage>,
    pub total_size: usize,
}

/// A single message in the IMAP maildrop.
struct ImapMessage {
    /// The lines of the message, including the terminating `"\r\n"`. Any lines starting with `.` are byte-stuffed.
    pub lines: Vec<String>,
    pub size: usize,
}
impl From<&Content> for ImapContent {
    fn from(content: &Content) -> Self {
        // Generate all pages of the site
        let mut pages = Vec::new();
        pages.push("From: Fletch\nTo: You!\nSubject: Welcome!\n\nHello! Welcome to my website, exposed via a IMAP mail server. All the pages should be listed here as emails, so feel free to browse around!".to_string());
        for project in content.projects.iter() {
            pages.push(format!(
                "From: Fletch\nTo: You!\nSubject: {}\n\n{}",
                project.name,
                project.to_string(),
            ));
        }

        // Convert pages to IMAP messages
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
            messages.push(ImapMessage {
                lines,
                size: message_size,
            });
            total_size += message_size;
        }
        ImapContent {
            messages,
            total_size,
        }
    }
}
