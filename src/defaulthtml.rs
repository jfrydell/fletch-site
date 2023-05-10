use std::{collections::HashMap, fs::read_to_string, sync::Arc};

use axum::{
    extract::Path,
    extract::{ws::Message, State},
    response::{Html, IntoResponse},
    routing::get,
    Router,
};
use liquid::{model::Value, Object};
use tokio::sync::RwLock;

use crate::project::Project;

/// Stores the rendered basic HTML content, for serving previews or writing to files.
pub struct DefaultHtmlContent {
    /// `index.html` contents
    pub index: String,
    /// Contents of `projects/` indexed by name
    pub projects: HashMap<String, String>,
    /// List of websockets to send to when content changes
    pub websockets: Vec<tokio::sync::mpsc::Sender<axum::extract::ws::Message>>,
}

impl DefaultHtmlContent {
    // Renders the basic HTML from the given content.
    pub fn new(content: &Vec<Project>) -> Result<Self, String> {
        Ok(Self {
            index: make_index(content)?,
            projects: make_projects(content)?,
            websockets: Vec::new(),
        })
    }

    // Rerender the basic HTML from the given content.
    pub async fn reload(&mut self, content: &Vec<Project>) -> Result<(), String> {
        // Reload content
        self.index = make_index(content)?;
        self.projects = make_projects(content)?;
        // Notify websockets and remove them, as they should reconnect with a new socket
        println!(
            "Notifying {} websockets of live-reload.",
            self.websockets.len()
        );
        for ws in self.websockets.drain(..) {
            ws.send(Message::Binary(vec![0]))
                .await
                .map_err(|e| format!("Failed to send websocket message: {}", e))?;
        }
        Ok(())
    }

    // Writes the basic HTML to the `defaulthtml/` directory.
    pub fn write(&self) -> Result<(), String> {
        // Write index
        std::fs::write("defaulthtml/index.html", &self.index)
            .map_err(|e| format!("Failed to write index.html file: {}", e))?;

        // Write projects
        for (url, html) in self.projects.iter() {
            std::fs::write(format!("defaulthtml/projects/{}.html", url), html)
                .map_err(|e| format!("Failed to write {}.html file: {}", url, e))?;
        }
        Ok(())
    }

    // Returns an axum router for serving basic HTML, which takes the `DefaultHtmlContent` as state.
    pub fn axum_router() -> Router<Arc<RwLock<Self>>> {
        Router::new()
            .route(
                "/",
                get(|State(content): State<Arc<RwLock<Self>>>| async move {
                    let content = content.read().await;
                    Html(Self::live_reload(content.index.clone()))
                }),
            )
            .route("/projects/*path", get(|Path(path): Path<String>, State(content): State<Arc<RwLock<Self>>>| async move {
                let content = content.read().await;
                content.projects.get(&path).map(|s| Html(Self::live_reload(s.clone()))).ok_or(format!("Bad Path: {} ({:?})", path, content.projects.keys()))
            }))
            .route("/ws", get(Self::ws_handler))
    }
    /// Wraps the given HTML in a live-reload script using websockets.
    fn live_reload(html: String) -> String {
        format!(
            r#"{html}
            <script>
                var socket = new WebSocket("ws://localhost:3000/defaulthtml/ws");
                socket.onmessage = function (event) {{
                    location.reload();
                }};
            </script>
            "#
        )
    }
    /// Handles websocket connections, adding them to a queue to update when content changes.
    async fn ws_handler(
        ws: axum::extract::ws::WebSocketUpgrade,
        State(content): State<Arc<RwLock<Self>>>,
    ) -> impl IntoResponse {
        ws.on_upgrade(|mut socket| async move {
            // Create a channel for sending messages to the websocket.
            let (tx, mut rx) = tokio::sync::mpsc::channel(1);
            // Add the channel to the list of websockets to send to when content changes.
            // After one live-reload message is sent, it will be removed, as the client should reconnect with a new socket.
            let mut content = content.write().await;
            content.websockets.push(tx);
            println!("Socket connected, listening for live-reloads.");
            tokio::spawn(async move {
                if let Some(msg) = rx.recv().await {
                    socket.send(msg).await.unwrap_or_else(|e| {
                        println!("Failed to send live-reload to socket: {e}");
                    });
                }
            });
        })
    }
}

/// Creates the Default HTML index.html page, returning either its contents on an Error.
fn make_index(projects: &Vec<Project>) -> Result<String, String> {
    // Create content object that template expects
    let mut obj = Object::new();
    obj.insert(
        "projects".into(),
        Value::array(projects.iter().map(|p| Value::Object(p.to_liquid()))),
    );

    // Build template
    let template = liquid::ParserBuilder::with_stdlib()
        .build()
        .unwrap()
        .parse(
            &read_to_string("defaulthtml-templates/index.liquid").map_err(|e| {
                format!(
                    "Failed to read template file `defaulthtml-templates/index.liquid`: {}",
                    e
                )
            })?,
        )
        .unwrap();

    // Render index.html
    let html = template.render(&obj).map_err(|e| {
        format!(
            "Failed to render template `defaulthtml-templates/index.liquid`: {}",
            e
        )
    })?;

    Ok(html)
}

/// Creates the Default HTML page for each project.
fn make_projects(projects: &Vec<Project>) -> Result<HashMap<String, String>, String> {
    // Build template
    let template = liquid::ParserBuilder::with_stdlib()
        .build()
        .unwrap()
        .parse(
            &read_to_string("defaulthtml-templates/project.liquid").map_err(|e| {
                format!(
                    "Failed to read template file `defaulthtml-templates/project.liquid`: {}",
                    e
                )
            })?,
        )
        .unwrap();

    // Process projects into HashMap of HTML
    let mut result = HashMap::new();
    for project in projects {
        // Create content object that template expects
        let mut obj = Object::new();
        obj.insert("project".into(), Value::Object(project.to_liquid()));

        // Render project.html
        let html = template
            .render(&obj)
            .map_err(|e| format!("Failed to render template `defaulthtml-templates/project.liquid` with project '{}': {}", project.name, e))?;

        // Add to hashmap
        result.insert(project.url.clone(), html);
    }
    Ok(result)
}
