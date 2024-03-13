use std::{convert::Infallible, net::SocketAddr, sync::Mutex};

use color_eyre::Result;

use tokio_rusqlite::Connection;
use tracing::error;
type SqlResult<T> = rusqlite::Result<T>;

/// A SQL connection to use for async queries; can be cheaply cloned while sharing one underlying connection in a separate thread.
static CONN: Mutex<Option<Connection>> = Mutex::new(None);

/// Sets up the messages database for the contact page at startup, then returns pending forever. Continues indefinitely after that (returning pending) while holding DB connection so we close connection on program exit via cancellation.
pub async fn main() -> Result<Infallible> {
    // Initialize DB
    let conn = Connection::open(&crate::CONFIG.msg_database).await?;
    conn.call(|conn| {
        // Global config (enable foreign keys if not already on)
        conn.pragma_update(None, "foreign_keys", "ON")?;

        // Create tables for threads and messages, along with index to speed up foreign key lookups (e.g. avoid full messages scan when deleting thread)
        conn.execute(
            "CREATE TABLE IF NOT EXISTS threads (
                id          INTEGER PRIMARY KEY,
                source_ip   TEXT NOT NULL,
                unread      INTEGER NOT NULL DEFAULT 0
            );",
            (),
        )?;
        conn.execute(
            "CREATE TABLE IF NOT EXISTS messages (
                thread      INTEGER NOT NULL REFERENCES threads(id) ON DELETE CASCADE ON UPDATE CASCADE,
                contents    TEXT NOT NULL,
                response    INTEGER NOT NULL CHECK(response = 0 OR response = 1),
                time        INTEGER NOT NULL
            );",
            (),
        )?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS message_thread_index ON messages(thread);", ()
        )?;

        // Keep unread count up to date (mark all as read once responded to) as messages are inserted
        conn.execute(
            "CREATE TRIGGER IF NOT EXISTS unread_increment BEFORE INSERT ON messages WHEN (NEW.response = 0) BEGIN
                UPDATE threads SET unread = unread + 1 WHERE id = NEW.thread;
            END;",
            ()
        )?;
        conn.execute(
            "CREATE TRIGGER IF NOT EXISTS unread_reset AFTER INSERT ON messages WHEN (NEW.response = 1) BEGIN
                UPDATE threads SET unread = 0 WHERE id = NEW.thread;
            END;",
            ()
        )?;
        Ok(())
    })
    .await?;

    // Set `CONN`, making copy so we still hold `conn` locally.
    *CONN.lock().expect("poison") = Some(conn.clone());

    // test
    for i in 0..10 {
        dbg!(create_thread(SocketAddr::from(([127, 0, 0, 1], 1)), format!("test {i}")).await);
    }
    for i in 0..10 {
        dbg!(create_thread(SocketAddr::from(([127, 0, 0, 2], 1)), format!("test {i}")).await);
    }

    Ok(futures::future::pending().await)
}

/// Possible errors that can occur while sending a message or thread.
#[derive(Debug)]
pub enum MessageError {
    /// An internal error occured with a database query and was logged internally.
    DatabaseError,
    /// The message is too large, exceeding `CONFIG.msg_max_size`
    TooLong,
    /// The thread already contains too many messages without a reply, exceeding `CONFIG.msg_max_unread_messages`.
    ThreadFull,
    /// There are too many unread threads in my inbox, so rate limiting is in effect. This may be due to the sending IP exceeding `CONFIG.msg_max_unread_threads_ip`, or all users exceeding `CONFIG.msg_max_unread_threads_global`.
    InboxFull,
}

pub async fn create_thread(ip: SocketAddr, first_message: String) -> Result<i64, MessageError> {
    // Get connection
    let conn = CONN
        .lock()
        .expect("poison")
        .clone()
        .ok_or(MessageError::DatabaseError)?;

    // Check message size
    if first_message.chars().count() > crate::CONFIG.msg_max_size {
        return Err(MessageError::TooLong);
    }

    // Rest of action is single transaction updating database, just send entire thing to background thread (could separately begin transaction, check validity, and write, but silly to do here since only have one connection anyway and if Sqlite is bottleneck have more to think about)
    conn.call(move |conn| {
        // Start transaction
        let tx = conn.transaction()?;

        // Check number of unread messages globally and per IP, returning error (but not database error) if checks fail
        if let Err(e) = check_unread_thread_count(&tx)? {
            return Ok(Err(e));
        }
        if let Err(e) = check_unread_thread_count_ip(&tx, ip)? {
            return Ok(Err(e));
        }

        // Generate random ID for thread
        let thread_id: i64 = rand::random();

        // Create thread and add message
        tx.execute(
            "INSERT INTO threads (id, source_ip) VALUES (?1, ?2);",
            (thread_id, ip.to_string()),
        )?;
        add_message(&tx, thread_id, first_message)?;

        // Commit transaction if no errors occurred (will rollback if thread count checks fail in addition to on database errors, which is fine as we haven't written and don't want to write)
        tx.commit()?;

        // Return thread ID if everything was successful (no database error, no `MessageError`)
        Ok(Ok(thread_id))
    })
    .await
    .unwrap_or_else(|err| {
        error!("Database error on thread creation: {err}");
        Err(MessageError::DatabaseError)
    })
}

/// Adds a message to the given thread (always setting `response = 0` and the time to Sqlite's current time), not checking any constraints.
///
/// Like all utilities that follow, this is a non-`async` method to run on `rusqlite::Connection`s within closures sent via `tokio_rusqlite`, rather than sending such a closure via the async interface within this function.
fn add_message(conn: &rusqlite::Connection, thread_id: i64, message: String) -> SqlResult<()> {
    conn.execute(
        "INSERT INTO messages (thread, contents, response, time) VALUES (?1, ?2, 0, unixepoch())",
        (thread_id, message),
    )
    .map(|_| ())
}

/// Gets the number of threads with unread messages, checking if we've exceeded `CONFIG.msg_max_unread_threads_global`. Returns `Ok(count)` if the count is within the allowed range, and `Err(MessageError::InboxFull)` otherwise.
///
/// This should be done in a transaction to avoid TOCTOU races.
fn check_unread_thread_count(
    conn: &rusqlite::Connection,
) -> SqlResult<Result<usize, MessageError>> {
    // Get count from database connection
    let count: usize = conn.query_row(
        "SELECT COUNT(*) FROM threads WHERE (unread > 0);",
        (),
        |row| row.get(0),
    )?;
    // Check count is under max
    Ok(if count >= crate::CONFIG.msg_max_unread_threads_global {
        Err(MessageError::InboxFull)
    } else {
        Ok(count)
    })
}

/// Gets the number of threads with unread messages for the given IP, checking if we've exceeded `CONFIG.msg_max_unread_threads_ip`. Returns `Ok(count)` if the count is within the allowed range, and `Err(MessageError::InboxFull)` otherwise (using the same error type to keep rate limiting somewhat opaque to user, especially as other users on IP could clog connection).
fn check_unread_thread_count_ip(
    conn: &rusqlite::Connection,
    ip: SocketAddr,
) -> SqlResult<Result<usize, MessageError>> {
    // Get count from database connection
    let count: usize = conn.query_row(
        "SELECT COUNT(*) FROM threads WHERE (unread > 0 AND source_ip = ?1);",
        [ip.to_string()],
        |row| row.get(0),
    )?;
    // Check count is under max
    Ok(if count >= crate::CONFIG.msg_max_unread_threads_ip {
        Err(MessageError::InboxFull)
    } else {
        Ok(count)
    })
}
