use std::{convert::Infallible, net::SocketAddr, sync::Arc};

use anyhow::Result;
use axum::{
    extract::{ws, Path, State},
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
pub async fn main(rx: broadcast::Receiver<()>) -> Result<Infallible> {
    // Create initial server
    let server = Arc::new(HtmlServer::new(&crate::CONTENT.read().unwrap()).await?);

    // Run server, global change listener, and local change listener. If any of them return an error, return it.
    tokio::select!(
        e = Arc::clone(&server).run() => e,
        e = server.listen_global_changes(rx) => e,
        e = server.listen_local_changes() => e,
    )
}

/// Holds all state needed by the Axum router, exposing it through interior mutability for access for reloads.
pub struct HtmlServer {
    /// Content to serve
    content: RwLock<HtmlContent>,
    /// Broadcaster that sends a message to all connected websockets
    websocket_tx: broadcast::Sender<()>,
}
impl HtmlServer {
    async fn new(content: &crate::Content) -> Result<Self> {
        Ok(Self {
            content: RwLock::new(HtmlContent::new(content).await?),
            websocket_tx: broadcast::channel(1).0,
        })
    }

    /// Run the server, running forever unless an error occurs.
    async fn run(self: Arc<Self>) -> Result<Infallible> {
        axum::Server::bind(&SocketAddr::from(([0, 0, 0, 0], 3000)))
            .serve(self.router().into_make_service())
            .await?;
        #[allow(unreachable_code)]
        Ok(unreachable!(
            "Server shouldn't shutdown unless an error occurs"
        ))
    }

    /// Listens for global content changes from the broadcast channel, reloading when they occur.
    async fn listen_global_changes(&self, mut rx: broadcast::Receiver<()>) -> Result<Infallible> {
        loop {
            match rx.recv().await {
                Ok(_) => {}
                Err(broadcast::error::RecvError::Closed) => {
                    anyhow::bail!("Global content change broadcast channel closed");
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    println!("Html server lagging behind global content changes");
                    continue;
                }
            };
            match self.refresh_content(&crate::CONTENT.read().unwrap()).await {
                Ok(_) => println!("Reloaded HTML content"),
                Err(e) => println!("Failed to reload HTML content: {e}"),
            }
            self.reload_clients();
        }
    }

    /// Listens for local content (template) changes, hard reloading when they occur.
    async fn listen_local_changes(&self) -> Result<Infallible> {
        crate::watch_path(std::path::Path::new("defaulthtml-templates/"), || async {
            self.refresh_content_hard(&crate::CONTENT.read().unwrap())
                .await?;
            self.reload_clients();
            Ok(())
        })
        .await
    }

    /// Creates the Axum router for the HTML server.
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
    fn reload_clients(&self) {
        let n = match self.websocket_tx.send(()) {
            Ok(n) => n,
            Err(_) => 0,
        };
        println!("Reloaded {n} clients");
    }

    /// Handles websocket connections, adding them to a queue to update when content changes.
    async fn ws_handler(
        ws: ws::WebSocketUpgrade,
        State(server): State<Arc<Self>>,
    ) -> impl IntoResponse {
        // Subscribe to the broadcast channel for websocket events
        let mut reload_rx = server.websocket_tx.subscribe();

        // Once the ws is ready, listen for events on the channel
        ws.on_upgrade(|socket| async move {
            use futures::{SinkExt, StreamExt};
            println!("Socket connected, listening for live-reloads.");

            // Split the socket into a sender and receiver
            let (mut socket_tx, mut socket_rx) = socket.split();

            // Wait for reload event or socket close
            tokio::select!(
                _ = reload_rx.recv() => {
                    socket_tx
                    .send(ws::Message::Binary(vec![]))
                    .await
                    .unwrap_or_else(|e| {
                        println!("Failed to send live-reload to socket: {e}");
                    });
                }
                _ = async {
                    while let Some(m) = socket_rx.next().await {
                        if matches!(m, Ok(ws::Message::Close(_))) {
                            break;
                        }
                    }
                } => {
                    println!("Reload socket closed");
                }
            );
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
            "</head>",
            r#"<script>
                const ws = new WebSocket(`ws://${window.location.host}/ws`);
                ws.onmessage = () => window.location.reload();
            </script>
            </head>"#,
        );
        Html(body).into_response()
    } else {
        response
    }
}
