use std::{convert::Infallible, future::Future, sync::RwLock};

use color_eyre::{eyre::eyre, Result};
use serde::Serialize;
use tokio::sync::broadcast;
use tracing::{debug, error, info};

mod html;
mod project;
mod ssh;

#[derive(Serialize)]
pub struct Content {
    projects: Vec<project::Project>,
    index_info: serde_json::Value,
    themes_info: serde_json::Value,
}
impl Content {
    /// Loads all content from the `content/` directory.
    async fn load() -> Result<Content> {
        // Get list of all projects from `content/projects`
        let mut projects = Vec::new();
        let mut entries = tokio::fs::read_dir("content/projects").await.unwrap();
        while let Some(entry) = entries.next_entry().await.unwrap() {
            let path = entry.path();
            if path.is_file() {
                // Load project
                let project: project::Project = quick_xml::de::from_reader(
                    std::io::BufReader::new(std::fs::File::open(path)?),
                )?;
                info!("Loaded project: {}", project.name);
                projects.push(project);
            }
        }
        projects.sort_by_key(|p| -p.priority);

        // Load index and themes info
        let index_info =
            serde_json::from_str(&tokio::fs::read_to_string("content/index.json").await?)?;
        let themes_info =
            serde_json::from_str(&tokio::fs::read_to_string("content/themes.json").await?)?;

        Ok(Content {
            projects,
            index_info,
            themes_info,
        })
    }
}

static CONTENT: RwLock<Content> = RwLock::new(Content {
    projects: Vec::new(),
    index_info: serde_json::Value::Null,
    themes_info: serde_json::Value::Null,
});

#[tokio::main]
async fn main() -> Result<Infallible> {
    // Set up error handling and logging
    if std::env::var("RUST_LIB_BACKTRACE").is_err() {
        std::env::set_var("RUST_LIB_BACKTRACE", "1");
    }
    color_eyre::install()?;
    if std::env::var("RUST_LOG").is_err() {
        std::env::set_var("RUST_LOG", "debug,hyper=warn");
    }
    tracing_subscriber::fmt::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    // Load initial content
    *CONTENT.write().unwrap() = Content::load().await.expect("Failed to load content");

    // Create broadcast channel for notifying services of content changes
    let (tx, rx) = broadcast::channel(1);

    // Run all services
    tokio::select!(
        e = html::main(rx.resubscribe()) => e,
        e = ssh::main(rx) => e,
        e = watch_content(tx) => e,
    )
}

/// Watches for changes to the shared `Content` and updates the static variable as needed. On update, sends a message on
/// a broadcast channel passed into this function.
async fn watch_content(broadcast_tx: broadcast::Sender<()>) -> Result<Infallible> {
    watch_path(std::path::Path::new("content/"), || async {
        // Load content, update static variable, and send message
        let content = Content::load().await?;
        info!("Content updated, broadcasting message");
        *CONTENT.write().unwrap() = content;
        broadcast_tx.send(()).unwrap_or_else(|e| {
            error!("No receivers for content update: {}", e);
            0
        });
        Ok(())
    })
    .await
}

/// Watches for changes to a path, running an async callback when they occur. If another change occurs during the callback's execution,
/// it is cancelled and retried.
pub async fn watch_path<F, Fut>(path: &std::path::Path, on_change: F) -> Result<Infallible>
where
    F: Fn() -> Fut,
    Fut: Future<Output = Result<()>>,
{
    use notify::{Config, Error, Event, RecommendedWatcher, RecursiveMode, Watcher};

    // Create an mpsc channel to send events to executor (allows verifying no new changes before sending broadcast)
    let (tx, mut rx) = tokio::sync::mpsc::channel(4);

    // Create a watcher object delivering all events via the mpsc channel
    let mut watcher = RecommendedWatcher::new(
        move |res: Result<Event, Error>| {
            tx.blocking_send(res.expect("Watcher error"))
                .expect("Watcher send failed")
        },
        Config::default(),
    )?;

    // Watch for changes
    watcher.watch(path, RecursiveMode::Recursive)?;
    info!("Listening for changes to {}", path.display());

    // Wait for events, running callback when they happen
    loop {
        // Wait for event, flushing all when one is seen
        rx.recv()
            .await
            .ok_or_else(|| eyre!("Watcher channel closed, can't receive filesystem events"))?;
        while rx.try_recv().is_ok() {}
        info!("Saw change to {}, reloading...", path.display());

        // Run callback, cancelling and retrying if another event occurs. If we keep seeing events for 1 second, stop cancelling and just go.
        let stop_retrying_time = std::time::Instant::now() + std::time::Duration::from_secs(1);
        loop {
            tokio::select! {
                biased;
                // Check to see if there's another event already, resetting the update if we see one
                _ = rx.recv(), if std::time::Instant::now() < stop_retrying_time => {
                    debug!("Change to {} mid-update, resetting update", path.display());
                    break;
                }
                // Run callback if no other event occurs
                result = on_change() => {
                    if let Err(e) = result {
                        error!("Error running change callback: {:?}", e);
                    }
                    break;
                }
            }
        }
    }
}
