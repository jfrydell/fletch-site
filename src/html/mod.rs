use std::{convert::Infallible, net::SocketAddr, sync::Arc};

use anyhow::Result;
use async_trait::async_trait;
use axum::{
    extract::{ws, FromRequestParts, Path, Query, State},
    response::{Html, IntoResponse},
    routing::get,
    Router,
};
use axum_extra::extract::{cookie::Cookie, CookieJar};
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, RwLock};
use tower_http::services::ServeDir;

mod defaulthtml;
mod simplehtml;

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
        crate::watch_path(std::path::Path::new("html-content/"), || async {
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
                get(
                    |State(server): State<Arc<Self>>, version: ExtractVersion| async move {
                        server.get_page(Page::Index, version).await
                    },
                ),
            )
            .route(
                "/projects/*path",
                get(
                    |State(server): State<Arc<Self>>,
                     Path(path): Path<String>,
                     version: ExtractVersion| async move {
                        server.get_page(Page::Project(path), version).await
                    },
                ),
            )
            .route("/ws", get(Self::ws_handler))
            .nest("/defaulthtml", defaulthtml::Content::router())
            .nest("/simplehtml", simplehtml::Content::router())
            .with_state(self)
            .nest_service("/images/", ServeDir::new("content/images/"))
    }

    /// Handles a request for a page, given which page and which version of content to use.
    async fn get_page(
        &self,
        page: Page,
        ExtractVersion(version, cookies): ExtractVersion,
    ) -> impl IntoResponse {
        // Logging
        println!("User requested page {page:?} with version {version:?}");

        // Get the page's content from the desired version
        let content = self.content.read().await;
        let response_body = match version {
            Some(HtmlVersion::DefaultHtml) => content.default.get_page(page),
            Some(HtmlVersion::SimpleHtml) => content.simple.get_page(page, false),
            Some(HtmlVersion::PureHtml) => content.simple.get_page(page, true),
            None => content.default.get_page(page),
        };

        // Serve page if possible, otherwise 404
        match response_body {
            Some(response_body) => {
                // Inject websocket script and serve
                let response_body = response_body.replace(
                    "</head>",
                    r#"<script>
                        const ws = new WebSocket(`ws://${window.location.host}/ws`);
                        ws.onmessage = () => window.location.reload();
                    </script>
                    </head>"#,
                );
                (cookies, Html(response_body)).into_response()
            }
            None => axum::http::StatusCode::NOT_FOUND.into_response(),
        }
    }

    /// Reloads the HTML content from scratch, rebuilding templates and populating general content.
    async fn refresh_content_hard(&self, new_content: &crate::Content) -> Result<()> {
        let new_content = HtmlContent::new(new_content).await?;
        *self.content.write().await = new_content;
        Ok(())
    }

    /// Reloads the HTML content based on the new general content, without reloading HTML templates.
    async fn refresh_content(&self, new_content: &crate::Content) -> Result<()> {
        self.content.write().await.refresh(new_content).await?;
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

/// An extractor getting the desired version of the HTML content along with possibly-updated cookies. If the version is `None`,
/// the default version should be used with a dialog to choose a version.
struct ExtractVersion(Option<HtmlVersion>, CookieJar);
#[async_trait]
impl<S> FromRequestParts<S> for ExtractVersion
where
    S: Send + Sync,
{
    type Rejection = Infallible;

    async fn from_request_parts(
        parts: &mut hyper::http::request::Parts,
        _state: &S,
    ) -> Result<Self, Self::Rejection> {
        use axum::RequestPartsExt;

        // Get the cookies from the request to see which version the user has set, if any.
        let cookies: CookieJar = parts.extract().await?;
        let version: Option<HtmlVersion> =
            cookies.get("version").and_then(|c| c.value().parse().ok());

        // Get the version from the query string, if any, overriding and setting a new cookie.
        #[derive(Deserialize)]
        struct QueryVersion {
            version: HtmlVersion,
        }
        match parts.extract::<Query<QueryVersion>>().await {
            Ok(Query(QueryVersion { version })) => Ok(Self(
                Some(version),
                cookies.add(Cookie::new("version", version.to_string())),
            )),
            Err(_) => Ok(Self(version, cookies)),
        }
    }
}

/// Holds all the HTML content, ready to be served. The `HtmlServer` and main thread share ownership of this.
struct HtmlContent {
    pub default: defaulthtml::Content,
    pub simple: simplehtml::Content,
}

impl HtmlContent {
    /// Renders the HTML content based on the given general content, from scratch.
    async fn new(content: &crate::Content) -> Result<Self> {
        let (default, simple) = tokio::try_join!(
            defaulthtml::Content::new(content),
            simplehtml::Content::new(content)
        )?;
        Ok(Self { default, simple })
    }

    /// Reloads the HTML content based on the given general content, without recreating the HTML content object itself.
    /// This should be used when the general content changes, but the HTML specific content (templates, etc.) does not.
    async fn refresh(&mut self, content: &crate::Content) -> Result<()> {
        self.default.refresh(content).await?;
        self.simple.refresh(content).await?;
        Ok(())
    }
}

/// The possible versions of the HTML content.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum HtmlVersion {
    #[serde(rename = "default")]
    DefaultHtml,
    #[serde(rename = "simple")]
    SimpleHtml,
    #[serde(rename = "pure")]
    PureHtml,
}
impl ToString for HtmlVersion {
    fn to_string(&self) -> String {
        match self {
            Self::DefaultHtml => "default".to_string(),
            Self::SimpleHtml => "simple".to_string(),
            Self::PureHtml => "pure".to_string(),
        }
    }
}
impl std::str::FromStr for HtmlVersion {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "default" => Ok(Self::DefaultHtml),
            "simple" => Ok(Self::SimpleHtml),
            "pure" => Ok(Self::PureHtml),
            _ => Err(()),
        }
    }
}

/// The possible pages we can serve.
#[non_exhaustive] // for future expansion (every lookup should return Option already, so easy to do)
#[derive(Debug, Clone)]
pub enum Page {
    Index,
    Project(String),
    BlogPost(String),
}
