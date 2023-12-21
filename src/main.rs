use std::{convert::Infallible, future::Future, sync::RwLock, time::Duration};

use base64::Engine;
use color_eyre::{eyre::eyre, Result};
use once_cell::sync::Lazy;
use tokio::sync::broadcast;
use tracing::{debug, error, info, log::warn};

mod blogpost;
mod content;
mod gopher;
mod html;
mod pop3;
mod project;
mod qotd;
mod ssh;

pub use content::Content;

pub static CONFIG: Lazy<Config> = Lazy::new(|| Config::load().expect("Failed to load config"));
#[derive(Debug)]
pub struct Config {
    /// Our domain name, shown in SSH prompts and some links.
    pub domain: String,
    /// The HTTP port to listen on.
    pub http_port: u16,
    /// The ssh port to listen on.
    pub ssh_port: u16,
    /// The ed25519 keypair to use for ssh.
    pub ssh_key: ed25519_dalek::Keypair,
    /// The timeout at which to close idle ssh connections (given in seconds).
    pub ssh_timeout: Duration,
    /// The first data timeout for ssh connections; new connections will be closed if no data is received within this time (given in seconds).
    pub ssh_first_timeout: Duration,
    /// The Gopher port to listen on.
    pub gopher_port: u16,
    /// The QOTD port to listen on.
    pub qotd_port: u16,
    /// The POP3 port to listen on.
    pub pop3_port: u16,
    /// Whether to watch for changes to the content directory (as well as any HTML templates) to update content.
    ///
    /// Currently affects all filesystem watching, but may be split into separate flags in the future.
    pub watch_content: bool,
    /// Whether to enable live reloading for HTTP clients on content changes.
    pub live_reload: bool,
    /// Whether to show hidden projects (those with priority <= 0).
    pub show_hidden: bool,
}
impl Config {
    /// Loads the config from env vars.
    fn load() -> Result<Self> {
        Ok(Self {
            domain: std::env::var("DOMAIN")?,
            http_port: Self::parse_var("HTTP_PORT")?,
            ssh_port: Self::parse_var("SSH_PORT")?,
            ssh_key: ed25519_dalek::Keypair::from_bytes(
                &base64::engine::general_purpose::STANDARD
                    .decode(
                        std::env::var("SSH_KEY")
                            .expect("Missing SSH_KEY env var")
                            .as_bytes(),
                    )
                    .expect("Invalid SSH_KEY env var (not base64)"),
            )
            .expect("Invalid SSH_KEY env var (not ed25519)"),
            ssh_timeout: std::time::Duration::from_secs(Self::parse_var_default(
                "SSH_TIMEOUT",
                30,
            )?),
            ssh_first_timeout: std::time::Duration::from_secs(Self::parse_var_default(
                "SSH_FIRST_TIMEOUT",
                30,
            )?),
            gopher_port: Self::parse_var("GOPHER_PORT")?,
            qotd_port: Self::parse_var("QOTD_PORT")?,
            pop3_port: Self::parse_var("POP3_PORT")?,
            watch_content: Self::parse_var_default("WATCH_CONTENT", false)?,
            live_reload: Self::parse_var_default("LIVE_RELOAD", false)?,
            show_hidden: Self::parse_var_default("SHOW_HIDDEN", false)?,
        })
    }
    /// Helper to load an env var, returning an error if it's missing or invalid
    fn parse_var<T>(var: &str) -> Result<T>
    where
        T: std::str::FromStr + std::fmt::Display,
        <T as std::str::FromStr>::Err: std::fmt::Display,
    {
        std::env::var(var)
            .map_err(|e| eyre!("Missing {} env var: {}", var, e))?
            .parse()
            .map_err(|e| eyre!("Invalid {} env var: {}", var, e))
    }
    /// Helper to load an env var, logging a warning but returning a default value if it's missing and returning an error if it's invalid.
    fn parse_var_default<T>(var: &str, default: T) -> Result<T>
    where
        T: std::str::FromStr + std::fmt::Display,
        <T as std::str::FromStr>::Err: std::fmt::Display,
    {
        match std::env::var(var) {
            Ok(v) => match v.parse() {
                Ok(v) => Ok(v),
                Err(e) => Err(eyre!("Invalid {} env var: {}", var, e)),
            },
            Err(_) => {
                warn!("Missing {} env var, defaulting to {}", var, default);
                Ok(default)
            }
        }
    }
    /// Logs all non-sensitive config values at debug level.
    fn log(&self) {
        let Self {
            domain,
            http_port,
            ssh_port,
            ssh_timeout,
            ssh_first_timeout,
            gopher_port,
            qotd_port,
            pop3_port,
            watch_content,
            live_reload,
            show_hidden,
            ssh_key: _,
        } = self;
        debug!("Config:");
        debug!("  DOMAIN: {}", domain);
        debug!("  HTTP_PORT: {}", http_port);
        debug!("  SSH_PORT: {}", ssh_port);
        debug!("  SSH_TIMEOUT: {}", ssh_timeout.as_secs());
        debug!("  SSH_FIRST_TIMEOUT: {}", ssh_first_timeout.as_secs());
        debug!("  GOPHER_PORT: {}", gopher_port);
        debug!("  QOTD_PORT: {}", qotd_port);
        debug!("  POP3_PORT: {}", pop3_port);
        debug!("  WATCH_CONTENT: {}", watch_content);
        debug!("  LIVE_RELOAD: {}", live_reload);
        debug!("  SHOW_HIDDEN: {}", show_hidden);
        debug!("End config.")
    }
}

static CONTENT: RwLock<Content> = RwLock::new(Content {
    projects: Vec::new(),
    blog_posts: Vec::new(),
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
        std::env::set_var("RUST_LOG", "debug,hyper=warn,russh=info");
    }
    tracing_subscriber::fmt::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    // Log config (partly to ensure it's loaded)
    CONFIG.log();

    // Load initial content
    *CONTENT.write().unwrap() = Content::load().await.expect("Failed to load content");

    // Create broadcast channel for notifying services of content changes
    let (tx, rx) = broadcast::channel(1);

    // Run all services
    let mut services = tokio::task::JoinSet::new();
    services.spawn(html::main(rx.resubscribe()));
    services.spawn(ssh::main(rx.resubscribe()));
    services.spawn(gopher::main(rx.resubscribe()));
    services.spawn(qotd::main(rx.resubscribe()));
    services.spawn(pop3::main(rx.resubscribe()));
    services.spawn(watch_content(tx));
    services.join_next().await.unwrap()?
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

    if !CONFIG.watch_content {
        // If we're not watching content, just stop task (can't return because it's an endless task, but sleeping forever as good in `select!()`)
        return Ok(futures::future::pending::<Infallible>().await);
    }

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
