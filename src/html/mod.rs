use std::{convert::Infallible, net::SocketAddr, sync::Arc};

use async_trait::async_trait;
use axum::{
    extract::{ws, FromRequestParts, Path, Query, State},
    response::{AppendHeaders, Html, IntoResponse},
    routing::get,
    Router, ServiceExt,
};
use color_eyre::{eyre, Result};
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, RwLock};
use tower_http::{normalize_path::NormalizePath, services::ServeDir};
use tracing::{debug, error, info, warn};

mod contact;
mod defaulthtml;
mod fancyhtml;
mod feed;
mod simplehtml;

/// Runs the HTML service, given a broadcast channel to notify it of content changes.
pub async fn main(rx: broadcast::Receiver<()>) -> Result<Infallible> {
    // Create initial server
    let server = Arc::new(HtmlServer::new(&crate::CONTENT.read().unwrap())?);

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
    fn new(content: &crate::Content) -> Result<Self> {
        Ok(Self {
            content: RwLock::new(HtmlContent::new(content)?),
            websocket_tx: broadcast::channel(1).0,
        })
    }

    /// Run the server, running forever unless an error occurs.
    async fn run(self: Arc<Self>) -> Result<Infallible> {
        let sock_addr = SocketAddr::from(([0, 0, 0, 0], crate::CONFIG.http_port));

        // Start server over HTTP
        tracing::info!("listening on http://{}", crate::CONFIG.http_port);
        axum_server::bind(sock_addr)
            .serve(ServiceExt::<hyper::Request<axum::body::Body>>::into_make_service(self.router()))
            .await
            .expect("Unable to start server");

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
                    eyre::bail!("Global content change broadcast channel closed");
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    warn!("Html server lagging behind global content changes");
                    continue;
                }
            };
            debug!("Reloading HTML content...");
            match self.refresh_content().await {
                Ok(_) => info!("Reloaded HTML content"),
                Err(e) => error!("Failed to reload HTML content: {e}"),
            }
            self.reload_clients();
        }
    }

    /// Listens for local content (template) changes, hard reloading when they occur.
    async fn listen_local_changes(&self) -> Result<Infallible> {
        crate::watch_path(std::path::Path::new("html-content/"), || async {
            self.refresh_content_hard().await?;
            self.reload_clients();
            Ok(())
        })
        .await
    }

    /// Creates the Axum router for the HTML server.
    fn router(self: Arc<Self>) -> NormalizePath<Router> {
        // Build router for all normal pages
        let mut router = Router::new()
            .route(
                "/",
                get(
                    |State(server): State<Arc<Self>>, version: ExtractVersion| async move {
                        server.get_page(Page::Index, version).await
                    },
                ),
            )
            .route(
                "/themes",
                get(
                    |State(server): State<Arc<Self>>, version: ExtractVersion| async move {
                        server.get_page(Page::Themes, version).await
                    },
                ),
            )
            .route(
                "/contact",
                get(
                    |State(server): State<Arc<Self>>, version: ExtractVersion| async move {
                        server.get_page(Page::Contact(None), version).await
                    },
                ),
            )
            .route(
                "/contact/:thread",
                get(
                    |State(server): State<Arc<Self>>,
                     Path(thread): Path<String>,
                     version: ExtractVersion| async move {
                        server.get_page(Page::Contact(Some(thread)), version).await
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
            .route(
                "/blog/*path",
                get(
                    |State(server): State<Arc<Self>>,
                     Path(path): Path<String>,
                     version: ExtractVersion| async move {
                        server.get_page(Page::BlogPost(path), version).await
                    },
                ),
            )
            .nest("/defaulthtml", defaulthtml::Content::router())
            .nest("/simplehtml", simplehtml::Content::router())
            .nest("/fancyhtml", fancyhtml::Content::router())
            .route(
                "/feed",
                get(|State(server): State<Arc<Self>>| async move {
                    (
                        [(hyper::header::CONTENT_TYPE, "application/xml")],
                        server.content.read().await.feed.atom(),
                    )
                }),
            );
        // Add websocket handler if live reload is enabled
        if crate::CONFIG.live_reload {
            router = router.route("/ws", get(Self::ws_handler));
        }
        // Finish router with state, contact API, static, and logging
        let router = router
            .with_state(self)
            .nest("/api/message", contact::router())
            .nest_service("/images/", ServeDir::new("content/images/"))
            .layer(tower_http::trace::TraceLayer::new_for_http());
        // Redirect trailing slashes
        tower_http::normalize_path::NormalizePath::trim_trailing_slash(router)
    }

    /// Handles a request for a page, given which page and which version of content to use.
    async fn get_page(
        &self,
        page: Page,
        ExtractVersion(version, cookies): ExtractVersion,
    ) -> impl IntoResponse {
        // Logging
        info!("User requested page {page:?} with version {version:?}");

        // Get the page's content from the desired version
        let content = self.content.read().await;
        let response_body = match version {
            Some(HtmlVersion::DefaultHtml) => content.default.get_page(&page),
            Some(HtmlVersion::SimpleHtml) => content.simple.get_page(&page, false),
            Some(HtmlVersion::PureHtml) => content.simple.get_page(&page, true),
            Some(HtmlVersion::FancyHtml) => content.fancy.get_page(&page),
            None => content.default.get_page(&page),
        };
        // If the desired version doesn't have the page, try the default version but log error
        let response_body = match response_body {
            Some(response_body) => Some(response_body),
            None => match version {
                Some(HtmlVersion::DefaultHtml) => None,
                _ => match content.default.get_page(&page) {
                    Some(response_body) => {
                        error!("Desired version {version:?} missing page {page:?}, falling back to default version");
                        Some(response_body)
                    }
                    None => None,
                },
            },
        };

        // Serve page if possible, otherwise 404
        match response_body {
            Some(mut response_body) => {
                // Inject websocket script if necessary and serve
                if crate::CONFIG.live_reload {
                    response_body = response_body.replace(
                        "</head>",
                        r#"<script>
                        const ws = new WebSocket(`ws://${window.location.host}/ws`);
                        ws.onmessage = () => window.location.reload();
                    </script>
                    </head>"#,
                    );
                }
                (
                    cookies,
                    AppendHeaders([(
                        hyper::header::LINK,
                        format!("<{}>; rel=\"canonical\"", get_canonical_url(&page)),
                    )]),
                    Html(response_body),
                )
                    .into_response()
            }
            None => axum::http::StatusCode::NOT_FOUND.into_response(),
        }
    }

    /// Reloads the HTML content from scratch, rebuilding templates and populating general content.
    async fn refresh_content_hard(&self) -> Result<()> {
        let new_content = HtmlContent::new(&crate::CONTENT.read().unwrap())?;
        *self.content.write().await = new_content;
        Ok(())
    }

    /// Reloads the HTML content based on the new general content, without reloading HTML templates.
    async fn refresh_content(&self) -> Result<()> {
        self.content
            .write()
            .await
            .refresh(&crate::CONTENT.read().unwrap())?;
        Ok(())
    }

    /// Reloads all connected clients.
    fn reload_clients(&self) {
        if !crate::CONFIG.live_reload {
            return;
        }
        let n = self.websocket_tx.send(()).unwrap_or(0);
        info!("Reloaded {n} clients");
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
            debug!("Socket connected, listening for live-reloads.");

            // Split the socket into a sender and receiver
            let (mut socket_tx, mut socket_rx) = socket.split();

            // Wait for reload event or socket close
            tokio::select!(
                _ = reload_rx.recv() => {
                    socket_tx
                    .send(ws::Message::Binary(vec![]))
                    .await
                    .unwrap_or_else(|e| {
                        warn!("Failed to send live-reload to socket: {e}");
                    });
                }
                _ = async {
                    while let Some(m) = socket_rx.next().await {
                        if matches!(m, Ok(ws::Message::Close(_))) {
                            break;
                        }
                    }
                } => {
                    debug!("Reload socket closed");
                }
            );
        })
    }
}

/// An extractor getting the desired version of the HTML content along with possibly-updated cookies. If the version is `None`,
/// the default version should be used with a dialog to choose a version.
struct ExtractVersion(Option<HtmlVersion>, axum_extra::extract::CookieJar);
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
        use axum_extra::extract::{cookie::Cookie, CookieJar};

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
                cookies.add(
                    Cookie::build(("version", version.to_string()))
                        .path("/")
                        .permanent(),
                ),
            )),
            Err(_) => Ok(Self(version, cookies)),
        }
    }
}

/// Holds all the HTML content, ready to be served. The `HtmlServer` and main thread share ownership of this.
///
/// The instructions for adding a new version are listed under `HtmlVersion`.
struct HtmlContent {
    pub default: defaulthtml::Content,
    pub simple: simplehtml::Content,
    pub fancy: fancyhtml::Content,
    pub feed: feed::Feed,
}

impl HtmlContent {
    /// Renders the HTML content based on the given general content, from scratch.
    fn new(content: &crate::Content) -> Result<Self> {
        Ok(Self {
            default: defaulthtml::Content::new(content)?,
            simple: simplehtml::Content::new(content)?,
            fancy: fancyhtml::Content::new(content)?,
            feed: feed::Feed::new(content)?,
        })
    }

    /// Reloads the HTML content based on the given general content, without recreating the HTML content object itself.
    /// This should be used when the general content changes, but the HTML specific content (templates, etc.) does not.
    fn refresh(&mut self, content: &crate::Content) -> Result<()> {
        self.default.refresh(content)?;
        self.simple.refresh(content)?;
        self.fancy.refresh(content)?;
        self.feed.refresh(content)?;
        Ok(())
    }
}

/// The possible versions of the HTML content.
///
/// When adding a new version, the following must be done:
/// - Add a new variant to `HtmlVersion`
///     - Update `FromStr` and `ToStr` implementations
/// - Add a new field to `HtmlContent`
///     - Update `new` and `refresh` methods
/// - Add a new match arm to `HtmlServer::get_page`
/// - Add a new nested router to `HtmlServer::router` (if needed)
///     - If another version was copy-pasted, update the nested router to extract the correct state from the `Arc<HtmlServer>`
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum HtmlVersion {
    #[serde(rename = "default")]
    DefaultHtml,
    #[serde(rename = "simple")]
    SimpleHtml,
    #[serde(rename = "pure")]
    PureHtml,
    #[serde(rename = "fancy")]
    FancyHtml,
}
impl ToString for HtmlVersion {
    fn to_string(&self) -> String {
        match self {
            Self::DefaultHtml => "default".to_string(),
            Self::SimpleHtml => "simple".to_string(),
            Self::PureHtml => "pure".to_string(),
            Self::FancyHtml => "fancy".to_string(),
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
            "fancy" => Ok(Self::FancyHtml),
            _ => Err(()),
        }
    }
}

/// The possible pages we can serve.
#[non_exhaustive] // for future expansion (every lookup should return Option already, so easy to do)
#[derive(Debug, Clone)]
pub enum Page {
    Index,
    Themes,
    Contact(Option<String>),
    Project(String),
    BlogPost(String),
}

fn get_canonical_url(page: &Page) -> String {
    match page {
        Page::Index => format!("https://{}/", crate::CONFIG.domain),
        Page::Themes => format!("https://{}/themes", crate::CONFIG.domain),
        Page::Contact(None) => format!("https://{}/contact", crate::CONFIG.domain),
        Page::Contact(Some(thread)) => {
            format!("https://{}/contact/{}", crate::CONFIG.domain, thread)
        }
        Page::Project(project) => format!("https://{}/projects/{}", crate::CONFIG.domain, project),
        Page::BlogPost(post) => format!("https://{}/blog/{}", crate::CONFIG.domain, post),
    }
}
