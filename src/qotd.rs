use std::convert::Infallible;

use color_eyre::Result;
use rand::seq::SliceRandom;
use tokio::{io::AsyncWriteExt, net::TcpListener, sync::broadcast};
use tracing::{error, info};

/// Runs the QOTD server, updating the content on `update_rx`.
pub async fn main(mut update_rx: broadcast::Receiver<()>) -> Result<Infallible> {
    // The possible quotes to send (kept in an `Arc` for sending to handler threads)
    let mut possible_quotes = generate_quotes(&crate::CONTENT.read().unwrap())?;
    // Initialize listeners for quote requests (currently just TCP)
    let tcp_listener = TcpListener::bind(("0.0.0.0", crate::CONFIG.qotd_port)).await?;
    // Handle quote requests and updates
    loop {
        tokio::select! {
            result = tcp_listener.accept() => {
                // Handle new connection
                let (mut stream, addr) = result?;
                info!("QOTD request (TCP) from {}", addr);
                // Select quote
                let quote = possible_quotes.choose(&mut rand::thread_rng()).unwrap().clone();
                // Spawn task to send quote
                tokio::task::spawn(async move {
                    if let Err(e) = stream.write_all(quote.as_bytes()).await {
                        error!("Error sending QOTD to {}: {}", addr, e);
                    }
                });
            }
            _ = update_rx.recv() => {
                // Reload content
                possible_quotes = generate_quotes(&crate::CONTENT.read().unwrap())?;
            }
        }
    }
}

/// Gets some possible quotes from the content.
pub fn generate_quotes(content: &crate::Content) -> Result<Vec<String>> {
    let mut quotes = Vec::new();
    for project in &content.projects {
        for line in project
            .to_string()
            .lines()
            .filter(|line| line.contains("."))
            .flat_map(|line| line.split("."))
            .filter(|q| !q.is_empty())
        {
            let quote = format!("From Project \"{}\":\n\"{}.\"\n", project.name, line.trim());
            if quote.len() < 512 {
                quotes.push(quote);
            }
        }
    }
    Ok(quotes)
}
