use std::{convert::Infallible, net::SocketAddr, sync::Mutex};

use color_eyre::Result;

use rusqlite::{OptionalExtension, TransactionBehavior};
use serde::Serialize;
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

    // Set `CONN` and make guard to unset/drop when cancelled (TODO: is this pointless? connection closed when file descriptor drops at process exit anyway? and not sure if dropping connection actually does anything either, despite docs claiming it does? ideally would close connection in thread, but tokio_rusqlite doesn't support).
    *CONN.lock().expect("poison") = Some(conn);
    struct Guard;
    impl Drop for Guard {
        fn drop(&mut self) {
            CONN.lock().expect("poison").take();
        }
    }
    let _guard = Guard;
    Ok(futures::future::pending().await)
}

/// Gets all messages on the given thread.
pub async fn get_messages(thread: ThreadId) -> Result<Vec<Message>, MessagesLoadError> {
    // Get connection and run rest of function in Sqlite thread
    let conn = CONN
        .lock()
        .expect("poison")
        .clone()
        .ok_or(MessagesLoadError::DatabaseError)?;
    conn
        .call(move |conn| {
            let tx = conn.transaction()?;

            // Check thread exists
            if tx.query_row(
                "SELECT COUNT(*) FROM threads WHERE id = ?1;",
                [thread.0],
                |row| row.get::<_, i32>(0),
            )? == 0
            {
                return Ok(Err(MessagesLoadError::NoSuchThread));
            }

            // Load messages
            let result: Vec<_> = tx.prepare_cached("SELECT contents, response, time FROM messages WHERE thread = ?1 ORDER BY time ASC;")?
                .query_map([thread.0], |row|
                    Ok(Message{
                        contents: row.get(0)?,
                        response: row.get(1)?,
                        timestamp: row.get(2)?,
                    })
                )?
                .collect::<Result<Vec<Message>,_>>()?;

            // Commit transaction and return results
            tx.commit()?;
            Ok(Ok(result))
        })
        .await
        .unwrap_or_else(|err| {
            error!("Database error on thread retrieval: {err}");
            Err(MessagesLoadError::DatabaseError)
        })
}

/// Creates a new thread of messages starting with the given one, returning the thread ID on success. Errors on database issues, a message exceeding the max size, or too many unresponded threads (globally or for the IP).
pub async fn create_thread(
    ip: SocketAddr,
    first_message: String,
) -> Result<ThreadId, MessageSendError> {
    // Get connection
    let conn = CONN
        .lock()
        .expect("poison")
        .clone()
        .ok_or(MessageSendError::DatabaseError)?;

    // Check message size
    if first_message.chars().count() > crate::CONFIG.msg_max_size {
        return Err(MessageSendError::TooLong);
    }

    // Normalize IP string representation
    let ip = ip.to_string();

    // Rest of action is single transaction updating database, just send entire thing to background thread (could separately begin transaction, check validity, and write, but silly to do here since only have one connection anyway and if Sqlite is bottleneck have more to think about).
    // TODO: doing everything at once atomically ensures only one transaction at a time, but if we don't, we need to consider concurrent writers, as our writes start out as read transactions due to validity check (UPDATE: added behavior mode immediate to fix)
    conn.call(move |conn| {
        // Start write transaction
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;

        // Check number of unread messages globally and per IP, returning error (but not database error) if checks fail
        if let Err(e) = check_unread_thread_count(&tx)? {
            return Ok(Err(e));
        }
        if let Err(e) = check_unread_thread_count_ip(&tx, &ip)? {
            return Ok(Err(e));
        }

        // Generate random ID for thread
        let thread_id = ThreadId(rand::random());

        // Create thread and add message
        tx.execute(
            "INSERT INTO threads (id, source_ip) VALUES (?1, ?2);",
            (thread_id.0, ip),
        )?;
        add_message(&tx, thread_id, first_message)?;

        // Commit transaction if no errors occurred (will rollback if thread count checks fail in addition to on database errors, which is fine as we haven't written and don't want to write)
        tx.commit()?;

        // Return thread ID if everything was successful (no database error, no `MessageSendError`)
        Ok(Ok(thread_id))
    })
    .await
    .unwrap_or_else(|err| {
        error!("Database error on thread creation: {err}");
        Err(MessageSendError::DatabaseError)
    })
}

/// Sends a message on the given thread. Errors on database issues or rate limiting as described by `MessageSendError` variants.
pub async fn send_message(thread_id: ThreadId, message: String) -> Result<(), MessageSendError> {
    // Get connection
    let conn = CONN
        .lock()
        .expect("poison")
        .clone()
        .ok_or(MessageSendError::DatabaseError)?;

    // Check message size
    if message.chars().count() > crate::CONFIG.msg_max_size {
        return Err(MessageSendError::TooLong);
    }

    // Rest of action is single transaction updating database, just send entire thing to background thread as in `create_thread`
    conn.call(move |conn| {
        // Start write transaction
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;

        // Check number of unread messages on this thread, verifying thread exists in the process and getting IP for next check
        let ip: String = match tx
            .query_row(
                "SELECT unread, source_ip FROM threads WHERE (id = ?1);",
                [thread_id.0],
                |row| Ok((row.get::<_, usize>(0)?, row.get(1)?)),
            )
            .optional()?
        {
            None => return Ok(Err(MessageSendError::NoSuchThread)),
            Some((c, _)) if c >= crate::CONFIG.msg_max_unread_messages => {
                return Ok(Err(MessageSendError::ThreadFull))
            }
            Some((_, ip)) => ip,
        };

        // Check number of unread messages globally and per IP, returning error (but not database error) if checks fail
        if let Err(e) = check_unread_thread_count(&tx)? {
            return Ok(Err(e));
        }
        if let Err(e) = check_unread_thread_count_ip(&tx, &ip)? {
            return Ok(Err(e));
        }

        // Actually send message
        add_message(&tx, thread_id, message)?;

        // Commit transaction if no errors occurred (will rollback if thread count checks fail in addition to on database errors, which is fine as we haven't written and don't want to write)
        tx.commit()?;

        // Return no database error and no `MessageSendError` for successful send
        Ok(Ok(()))
    })
    .await
    .unwrap_or_else(|err| {
        error!("Database error on thread creation: {err}");
        Err(MessageSendError::DatabaseError)
    })
}

/// Adds a message to the given thread (always setting `response = 0` and the time to Sqlite's current time), not checking any constraints.
///
/// Like all utilities that follow, this is a non-`async` method to run on `rusqlite::Connection`s within closures sent via `tokio_rusqlite`, rather than sending such a closure via the async interface within this function.
fn add_message(conn: &rusqlite::Connection, thread_id: ThreadId, message: String) -> SqlResult<()> {
    conn.execute(
        "INSERT INTO messages (thread, contents, response, time) VALUES (?1, ?2, 0, unixepoch())",
        (thread_id.0, message),
    )
    .map(|_| ())
}

/// Gets the number of threads with unread messages, checking if we've exceeded `CONFIG.msg_max_unread_threads_global`. Returns `Ok(count)` if the count is within the allowed range, and `Err(MessageSendError::InboxFull)` otherwise.
///
/// This should be done in a transaction to avoid TOCTOU races.
fn check_unread_thread_count(
    conn: &rusqlite::Connection,
) -> SqlResult<Result<usize, MessageSendError>> {
    // Get count from database connection
    let count: usize = conn.query_row(
        "SELECT COUNT(*) FROM threads WHERE (unread > 0);",
        (),
        |row| row.get(0),
    )?;
    // Check count is under max
    Ok(if count >= crate::CONFIG.msg_max_unread_threads_global {
        Err(MessageSendError::InboxFull)
    } else {
        Ok(count)
    })
}

/// Gets the number of threads with unread messages for the given IP, checking if we've exceeded `CONFIG.msg_max_unread_threads_ip`. Returns `Ok(count)` if the count is within the allowed range, and `Err(MessageSendError::InboxFull)` otherwise (using the same error type to keep rate limiting somewhat opaque to user, especially as other users on IP could clog connection).
fn check_unread_thread_count_ip(
    conn: &rusqlite::Connection,
    ip: &str,
) -> SqlResult<Result<usize, MessageSendError>> {
    // Get count from database connection
    let count: usize = conn.query_row(
        "SELECT COUNT(*) FROM threads WHERE (unread > 0 AND source_ip = ?1);",
        [ip.to_string()],
        |row| row.get(0),
    )?;
    // Check count is under max
    Ok(if count >= crate::CONFIG.msg_max_unread_threads_ip {
        Err(MessageSendError::InboxFull)
    } else {
        Ok(count)
    })
}

/// A wrapper for a thread ID, represented internally (for Sqlite) as an `i64`. Represented as case-insensitive twos-complement hexadecimal for the user.
#[derive(Clone, Copy, Debug)]
pub struct ThreadId(i64);
impl std::fmt::Display for ThreadId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:016x}", self.0)
    }
}
impl std::str::FromStr for ThreadId {
    type Err = std::num::ParseIntError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        u64::from_str_radix(s, 16).map(|i| ThreadId(i as i64))
    }
}

/// Represents a single message, including its contents, (unix) timestamp, and whether it was a response (from me; non-responses are from users).
#[derive(Clone, Debug, Serialize)]
pub struct Message {
    pub contents: String,
    pub timestamp: i64,
    pub response: bool,
}

/// Possible errors occurring when retrieving a thread's messages.
#[derive(Debug)]
pub enum MessagesLoadError {
    /// An internal error occured with a database query and was logged internally.
    DatabaseError,
    /// Tried to load a thread that doesn't exist.
    NoSuchThread,
}
impl std::fmt::Display for MessagesLoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MessagesLoadError::DatabaseError => write!(f, "internal server error, sorry :("),
            MessagesLoadError::NoSuchThread => write!(f, "invalid thread ID"),
        }
    }
}

/// Possible errors that can occur while sending a message or thread.
#[derive(Debug)]
pub enum MessageSendError {
    /// An internal error occured with a database query and was logged internally.
    DatabaseError,
    /// The message is too large, exceeding `CONFIG.msg_max_size`.
    TooLong,
    /// The thread already contains too many messages without a reply, exceeding `CONFIG.msg_max_unread_messages`.
    ThreadFull,
    /// There are too many unread threads in my inbox, so rate limiting is in effect. This may be due to the sending IP exceeding `CONFIG.msg_max_unread_threads_ip`, or all users exceeding `CONFIG.msg_max_unread_threads_global`.
    InboxFull,
    /// Tried to send a message on a thread that doesn't exist.
    NoSuchThread,
}
impl std::fmt::Display for MessageSendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MessageSendError::DatabaseError => write!(f, "internal server error, sorry :("),
            MessageSendError::TooLong => write!(
                f,
                "your message is too long (max size: {} characters)",
                crate::CONFIG.msg_max_size
            ),
            MessageSendError::ThreadFull => write!(
                f,
                "too many messages in a row without a reply (max {}), be patient!",
                crate::CONFIG.msg_max_unread_messages
            ),
            MessageSendError::InboxFull => write!(
                f,
                "sorry, I'm overwhelmed with unread messages right now, check back later"
            ),
            MessageSendError::NoSuchThread => write!(f, "invalid thread ID"),
        }
    }
}
