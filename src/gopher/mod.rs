use std::{
    convert::Infallible,
    io::{BufRead, BufReader, BufWriter, Write},
    sync::Arc,
};

use color_eyre::Result;
use gophermap::GopherMenu;
use tokio::{
    net::{TcpListener, TcpStream},
    sync::broadcast,
};
use tracing::{error, info};

use content::GopherContent;

mod content;

/// Runs the gopher server, updating the content on `update_rx`.
pub async fn main(mut update_rx: broadcast::Receiver<()>) -> Result<Infallible> {
    // To avoid locking the content during a slow request, we make a read-only copy of the content to serve from.
    // This is basically the same as the other presenters, but without our own version of the content (yet).
    let mut content = Arc::new(crate::CONTENT.read().unwrap().clone());
    let listener = TcpListener::bind(("0.0.0.0", crate::CONFIG.gopher_port)).await?;
    loop {
        tokio::select! {
            result = listener.accept() => {
                // Handle new connection
                let (stream, addr) = result?;
                info!("Gopher request from {}", addr);
                let content = Arc::clone(&content);
                tokio::task::spawn_blocking(move || {
                    handle(stream, content).unwrap_or_else(|e| {
                        error!("Error handling gopher request: {}", e);
                    })
                });
            }
            _ = update_rx.recv() => {
                // Relaod content
                content = Arc::new(crate::CONTENT.read().unwrap().clone());
            }
        }
    }
}

/// Handles one gopher request. TODO: non-blocking
pub fn handle(stream: TcpStream, content: Arc<crate::Content>) -> Result<()> {
    // TODO: timeout on reading full message
    let mut stream = stream.into_std()?;
    let mut selector = String::new();
    BufReader::new(stream.try_clone()?).read_line(&mut selector)?;
    let selector = selector.trim();

    // Match selector to find content to serve
    if selector.is_empty() || selector == "/" {
        let mut menu = GopherMenu::with_write(&mut stream);
        content.gopher(&menu)?;
        menu.end()?;
    } else if let Some(project) = selector.strip_prefix("/projects/") {
        // Serve project as either directory or TXT
        if let Some(project) = project.strip_suffix(".txt") {
            if let Some(project) = content.projects.iter().find(|p| p.url == project) {
                stream.write_all(project.to_string().as_bytes())?;
            } else {
                stream.write_all(b"Project not found")?;
            }
        } else {
            let mut menu = GopherMenu::with_write(&mut stream);
            if let Some(project) = content.projects.iter().find(|p| p.url == project) {
                project.gopher(&mut menu)?;
            } else {
                menu.info("Project not found")?;
                menu.write_entry(
                    gophermap::ItemType::Directory,
                    "Go Home",
                    "/",
                    &crate::CONFIG.domain,
                    crate::CONFIG.gopher_port,
                )?;
            }
            menu.end()?;
        }
    } else if let Some(image) = selector.strip_prefix("/images/") {
        // Serve image from content directory
        let image = std::path::Path::new("content/images/").join(image);
        if image.exists() {
            let mut file = std::fs::File::open(image)?;
            std::io::copy(&mut file, &mut BufWriter::new(stream))?;
        } else {
            stream.write_all(b"Image not found")?;
        }
    }

    Ok(())
}
