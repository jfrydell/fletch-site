use axum::Router;
use defaulthtml::DefaultHtmlContent;
use tower_http::services::ServeDir;

mod defaulthtml;
mod project;

#[tokio::main]
async fn main() {
    // TODO: handle startup better
    let mut defaulthtml_content = DefaultHtmlContent::new();
    defaulthtml_content
        .render(&load_content().unwrap())
        .await
        .unwrap();
    let defaulthtml_content = std::sync::Arc::new(tokio::sync::RwLock::new(defaulthtml_content));

    // Add watcher to update defaulthtml_content if any content/template changes (TODO: separate content changing from templates changing)
    tokio::spawn(watch_defaulthtml(defaulthtml_content.clone()));

    // Create Axum webserver to show preview
    let app = Router::new()
        .nest(
            "/defaulthtml/",
            DefaultHtmlContent::axum_router().with_state(defaulthtml_content),
        )
        .nest_service("/assets/images/", ServeDir::new("content/images/"));

    axum::Server::bind(&"0.0.0.0:3000".parse().unwrap())
        .serve(app.into_make_service())
        .await
        .unwrap();
}

/// Loads all content.
fn load_content() -> Result<Vec<project::Project>, String> {
    // Get list of all projects from `content/projects`
    let mut projects = Vec::new();
    for entry in std::fs::read_dir("content/projects").unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.is_file() {
            // Load project
            let project: project::Project = match serde_xml_rs::from_reader(
                std::io::BufReader::new(std::fs::File::open(path).unwrap()),
            ) {
                Ok(p) => p,
                Err(e) => {
                    println!("Failed to load project: {}", e);
                    continue;
                }
            };
            projects.push(project);
        }
    }
    projects.sort_by_key(|p| -p.priority);
    Ok(projects)
}

/// Watches for changes to the DefaultHTML content and templates, updating the rendered content as needed.
async fn watch_defaulthtml(
    defaulthtml_content: std::sync::Arc<tokio::sync::RwLock<DefaultHtmlContent>>,
) {
    use notify::{Config, RecommendedWatcher, RecursiveMode, Watcher};

    // Create a channel to send events to main task
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);

    // Create a watcher object delivering all events via the channel
    let mut watcher = RecommendedWatcher::new(
        move |res| tx.blocking_send(res).expect("Watcher send failed"),
        Config::default(),
    )
    .unwrap();
    watcher
        .watch(
            std::path::Path::new("content/projects/"),
            RecursiveMode::Recursive,
        )
        .unwrap();
    watcher
        .watch(
            std::path::Path::new("defaulthtml-templates/"),
            RecursiveMode::Recursive,
        )
        .unwrap();

    // Wait for events, reloading content when they happen
    while let Some(res) = rx.recv().await {
        let event = res.expect("Watcher error");
        let mut content = defaulthtml_content.write().await;
        println!("Updating content ({event:?})");
        content.render(&load_content().unwrap()).await.unwrap();
    }
}
