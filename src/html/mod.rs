use std::{
    net::SocketAddr,
    sync::{Arc, Mutex},
};

use anyhow::Result;
use axum::{
    extract::{Path, State},
    http::{header, Request},
    middleware::Next,
    response::{Html, IntoResponse},
    routing::get,
    Router,
};
use tokio::sync::{broadcast, RwLock};
use tower_http::services::ServeDir;

mod defaulthtml;

/// Runs the HTML service, given a broadcast channel to notify it of content changes.
pub async fn main(mut rx: broadcast::Receiver<()>) -> Result<()> {
    // Create the HTML server and Axum router (app)
    let server = Arc::new(HtmlServer::new(&crate::CONTENT.read().unwrap()).await?);
    let app = Arc::clone(&server).router();

    // Create a task to run the server
    let run_server = async {
        axum::Server::bind(&SocketAddr::from(([0, 0, 0, 0], 3000)))
            .serve(app.into_make_service())
            .await
            .map_err(|e| eprintln!("Server error: {}", e))
    };

    // Create a task to listen for global content changes, reloading when they occur
    let global_reload = async {
        while let Ok(_) = rx.recv().await {
            match server
                .refresh_content(&crate::CONTENT.read().unwrap())
                .await
            {
                Ok(_) => println!("Reloaded HTML content"),
                Err(e) => println!("Failed to reload HTML content: {e}"),
            }
            server.reload_clients().await;
        }
    };

    // Create a task to listen for local content (template) changes, hard reloading when they occur
    let local_reload =
        crate::watch_path(std::path::Path::new("defaulthtml-templates/"), || async {
            match server
                .refresh_content_hard(&crate::CONTENT.read().unwrap())
                .await
            {
                Ok(_) => println!("Hard-reloaded HTML content"),
                Err(e) => println!("Failed to hard-reload HTML content: {e}"),
            }
            server.reload_clients().await;
        });

    // Run server and live-reload tasks
    tokio::join!(run_server, global_reload, local_reload);

    Ok(())
}

/// Holds all state needed by the Axum router, exposing it through interior mutability for access for reloads.
pub struct HtmlServer {
    /// Content to serve
    content: RwLock<HtmlContent>,
    /// List of websockets to send to when content changes
    websockets: Mutex<Vec<tokio::sync::mpsc::Sender<axum::extract::ws::Message>>>,
}
impl HtmlServer {
    async fn new(content: &crate::Content) -> Result<Self> {
        Ok(Self {
            content: RwLock::new(HtmlContent::new(content).await?),
            websockets: Mutex::new(Vec::new()),
        })
    }

    fn router(self: Arc<Self>) -> Router {
        Router::new()
            .route(
                "/",
                get(|State(server): State<Arc<Self>>| async move {
                    Html(server.content.read().await.default.index.clone())
                }),
            )
            .route(
                "/projects/*path",
                get(
                    |Path(path): Path<String>, State(server): State<Arc<Self>>| async move {
                        let content = server.content.read().await;
                        content
                            .default
                            .projects
                            .get(&path)
                            .map(|s| Html(s.clone()))
                            .ok_or(format!(
                                "Bad Path: {} ({:?})",
                                path,
                                content.default.projects.keys()
                            ))
                    },
                ),
            )
            .layer(axum::middleware::from_fn(add_websocket_script))
            .route("/ws", get(Self::ws_handler))
            .nest("/defaulthtml", defaulthtml::Content::router())
            .with_state(self)
            .nest_service("/images/", ServeDir::new("content/images/"))
    }

    /// Reloads the HTML content from scratch, rebuilding templates and populating general content.
    async fn refresh_content_hard(&self, new_content: &crate::Content) -> Result<()> {
        let new_content = HtmlContent::new(new_content).await?;
        *self.content.write().await = new_content;
        Ok(())
    }

    /// Reloads the HTML content based on the new general content, without reloading HTML templates.
    async fn refresh_content(&self, new_content: &crate::Content) -> Result<()> {
        let mut content = self.content.write().await;
        content.refresh(new_content).await?;
        Ok(())
    }

    /// Reloads all connected clients.
    async fn reload_clients(&self) -> Result<()> {
        let ws: Vec<_> = std::mem::take(self.websockets.lock().unwrap().as_mut());
        for tx in ws {
            tx.send(axum::extract::ws::Message::Binary(vec![0])).await?;
        }
        Ok(())
    }

    /// Handles websocket connections, adding them to a queue to update when content changes.
    async fn ws_handler(
        ws: axum::extract::ws::WebSocketUpgrade,
        State(server): State<Arc<Self>>,
    ) -> impl IntoResponse {
        // Create a channel for sending messages to the websocket.
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        // Add the channel to the list of websockets to send to when content changes.
        // After one live-reload message is sent, it will be removed, as the client should reconnect with a new socket.
        server.websockets.lock().unwrap().push(tx);

        // Once the ws is ready, listen for events on the channel
        ws.on_upgrade(|mut socket| async move {
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

/// Holds all the HTML content, ready to be served. The `HtmlServer` and main thread share ownership of this.
struct HtmlContent {
    pub default: defaulthtml::Content,
}

impl HtmlContent {
    /// Renders the HTML content based on the given general content, from scratch.
    async fn new(content: &crate::Content) -> Result<Self> {
        Ok(Self {
            default: defaulthtml::Content::new(content).await?,
        })
    }

    /// Reloads the HTML content based on the given general content, without recreating the HTML content object itself.
    /// This should be used when the general content changes, but the HTML specific content (templates, etc.) does not.
    async fn refresh(&mut self, content: &crate::Content) -> Result<()> {
        self.default.refresh(content).await?;
        Ok(())
    }
}

// Adds a websocket script to any HTML responses, with the client reloading the page when a byte is received.
async fn add_websocket_script<B>(request: Request<B>, next: Next<B>) -> impl IntoResponse {
    let response = next.run(request).await;
    if response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .starts_with("text/html")
    {
        let body = response.into_body();
        let body = hyper::body::to_bytes(body).await.unwrap();
        let body = String::from_utf8(body.to_vec()).unwrap();
        let body = body.replace(
            "</body>",
            r#"<script>
                const ws = new WebSocket(`ws://${window.location.host}/ws`);
                ws.onmessage = () => window.location.reload();
            </script>
            </body>"#,
        );
        Html(body).into_response()
    } else {
        response
    }
}
