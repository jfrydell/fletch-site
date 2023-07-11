use std::{future::Future, sync::RwLock};

use serde::Serialize;
use tokio::sync::broadcast;

mod html;
mod project;
mod ssh;

#[derive(Default, Serialize)]
pub struct Content {
    projects: Vec<project::Project>,
}

static CONTENT: RwLock<Content> = RwLock::new(Content {
    projects: Vec::new(),
});

#[tokio::main]
async fn main() {
    // Load initial content
    *CONTENT.write().unwrap() = load_content().await.expect("Failed to load content");

    // Create broadcast channel for notifying services of content changes
    let (tx, rx) = broadcast::channel(1);

    // Run all services
    tokio::join!(html::main(rx), watch_content(tx));

    /*
    let mut defaulthtml_content = HtmlContent::new();
    let defaulthtml_content = std::sync::Arc::new(tokio::sync::RwLock::new(defaulthtml_content));

    // Start SSH. TODO: add live-reloading (note that SshContent is read-only for sessions for consistent state, must notify server? otherwise Arc<RwLock<Arc<_>>> so we can edit inner Arc???)
    let ssh_content = ssh::SshContent::new(&&CONTENT.read().unwrap());
    let ssh_content = std::sync::Arc::new(ssh_content);
    tokio::spawn(ssh::main(ssh_content)).await.unwrap();

    // Add watcher to update defaulthtml_content if any content/template changes (TODO: separate content changing from templates changing)
    tokio::spawn(watch_defaulthtml(defaulthtml_content.clone()));

    // Create Axum webserver to show preview
    let app = Router::new()
        .nest(
            "/defaulthtml/",
            DefaultHtmlContent::axum_router().with_state(defaulthtml_content.clone()),
        )
        .nest_service("/assets/images/", ServeDir::new("content/images/"));

    axum::Server::bind(&"0.0.0.0:3000".parse().unwrap())
        .serve(app.into_make_service())
        .await
        .unwrap();

     */
}

/// Loads all content.
async fn load_content() -> Result<Content, String> {
    // Get list of all projects from `content/projects`
    let mut projects = Vec::new();
    let mut entries = tokio::fs::read_dir("content/projects").await.unwrap();
    while let Some(entry) = entries.next_entry().await.unwrap() {
        let path = entry.path();
        if path.is_file() {
            // Load project
            let project: project::Project = match quick_xml::de::from_reader(
                std::io::BufReader::new(std::fs::File::open(path).unwrap()),
            ) {
                Ok(p) => p,
                Err(e) => {
                    println!("Failed to load project: {}", e);
                    continue;
                }
            };
            println!("Loaded project: {}", project.name);
            projects.push(project);
        }
    }
    projects.sort_by_key(|p| -p.priority);
    Ok(Content { projects })
}

/// Watches for changes to the shared `Content` and updates the static variable as needed. On update, sends a message on
/// a broadcast channel passed into this function.
async fn watch_content(broadcast_tx: broadcast::Sender<()>) -> ! {
    watch_path(std::path::Path::new("content/projects/"), || async {
        // Load content, update static variable, and send message
        load_content().await.expect("Failed to load content");
        println!("Content updated, broadcasting message");
        *CONTENT.write().unwrap() = load_content().await.unwrap();
        broadcast_tx.send(()).unwrap();
    })
    .await
}

/// Watches for changes to a path, running an async callback when they occur. If another change occurs during the callback's execution,
/// it is cancelled and retried.
pub async fn watch_path<F, Fut>(path: &std::path::Path, on_change: F) -> !
where
    F: Fn() -> Fut,
    Fut: Future<Output = ()>,
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
    )
    .unwrap();

    // Watch for changes
    watcher.watch(path, RecursiveMode::Recursive).unwrap();
    println!("Listening for changes to {}", path.display());

    // Wait for events, running callback when they happen
    loop {
        // Wait for event, flushing all when one is seen
        rx.recv().await.unwrap();
        while rx.try_recv().is_ok() {}
        println!("Saw change to {}, reloading...", path.display());

        // Run callback, cancelling and retrying if another event occurs. If we keep seeing events for 1 second, stop cancelling and just go.
        let stop_retrying_time = std::time::Instant::now() + std::time::Duration::from_secs(1);
        loop {
            tokio::select! {
                biased;
                // Check to see if there's another event already, resetting the update if we see one
                _ = rx.recv(), if std::time::Instant::now() < stop_retrying_time => {
                    println!("Change to {} mid-update, resetting update", path.display());
                    break;
                }
                // Update content if no other event occurs
                _ = on_change() => {
                    break;
                }
            }
        }
    }
}
