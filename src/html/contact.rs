use std::net::SocketAddr;

use axum::{
    extract::Path,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use hyper::StatusCode;
use tracing::error;

use crate::contact::{self, Message, MessageSendError, MessagesLoadError, ThreadId};

/// Gets a router to handle API calls for messaging.
pub fn router() -> Router {
    Router::new()
        .route("/send", post(create_thread))
        .route("/reply/:thread", post(send_message))
        .route("/load/:thread", get(get_messages))
}

/// Handles a POST request to create a new thread, returning the thread ID if successful and an error message otherwise.
async fn create_thread(msg: String) -> impl IntoResponse {
    let ip = SocketAddr::from(([0, 0, 0, 0], 0)); // i think nginx passes in header?
    error!("ip address in create_thread API");
    match contact::create_thread(ip, msg).await {
        Ok(id) => (StatusCode::OK, id.to_string()),
        Err(e) => ((&e).into(), format!("Error: {e}")),
    }
}

/// Handles a POST request to send a message on the given thread, returning an error message upon failure.
async fn send_message(Path(thread): Path<String>, msg: String) -> impl IntoResponse {
    // Parse thread ID
    let Ok(thread) = thread.parse::<ThreadId>() else {
        return (
            StatusCode::BAD_REQUEST,
            "Error: ill-formed thread ID (should be a 64-bit hexadecimal integer)".to_string(),
        ); // TODO: NOT_ACCEPTABLE? NOT_FOUND? check RFCs
    };

    // Send message and report result
    match crate::contact::send_message(thread, msg).await {
        Ok(()) => (StatusCode::OK, String::new()),
        Err(e) => ((&e).into(), format!("Error sending message: {e}")),
    }
}

/// Handles a GET request for the messages in a thread.
async fn get_messages(Path(thread): Path<String>) -> impl IntoResponse {
    // Helper macro wrapping error in response (felt cute, might delete later)
    macro_rules! wrap_error(
        ($msg:expr) => {
            Json(vec![Message {
                contents: $msg,
                timestamp: 0, // TODO: probably should update? or just leave, doesn't really matter
                response: true,
            }])
        }
    );

    // Parse thread ID
    let Ok(thread) = thread.parse::<ThreadId>() else {
        return (
            StatusCode::BAD_REQUEST,
            wrap_error!(
                "Error: ill-formed thread ID (should be a 64-bit hexadecimal integer)".to_string()
            ),
        ); // TODO: NOT_ACCEPTABLE? NOT_FOUND? check RFCs
    };

    // Get messages and return
    match contact::get_messages(thread).await {
        Ok(msgs) => (StatusCode::OK, Json(msgs)),
        Err(e) => ((&e).into(), wrap_error!(format!("Error: {e}"))),
    }
}

impl From<&MessageSendError> for StatusCode {
    fn from(err: &MessageSendError) -> Self {
        match err {
            MessageSendError::DatabaseError => StatusCode::INTERNAL_SERVER_ERROR,
            MessageSendError::TooLong => StatusCode::PAYLOAD_TOO_LARGE,
            MessageSendError::ThreadFull => StatusCode::TOO_MANY_REQUESTS,
            MessageSendError::InboxFull => StatusCode::INSUFFICIENT_STORAGE,
            MessageSendError::NoSuchThread => StatusCode::NOT_FOUND,
        }
    }
}

impl From<&MessagesLoadError> for StatusCode {
    fn from(err: &MessagesLoadError) -> Self {
        match err {
            MessagesLoadError::DatabaseError => StatusCode::INTERNAL_SERVER_ERROR,
            MessagesLoadError::NoSuchThread => StatusCode::NOT_FOUND,
        }
    }
}
