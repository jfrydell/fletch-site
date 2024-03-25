use axum::{
    extract::Path,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use axum_client_ip::{SecureClientIp, SecureClientIpSource};
use hyper::StatusCode;
use tracing::error;

use crate::contact::{self, MessageSendError, MessagesLoadError, ThreadId};

/// Gets a router to handle API calls for messaging.
pub fn router() -> Router {
    Router::new()
        .route("/send", post(create_thread))
        .route("/reply/:thread", post(send_message))
        .route("/load/:thread", get(get_messages))
        .layer(SecureClientIpSource::RightmostXForwardedFor.into_extension())
}

/// Handles a POST request to create a new thread, returning the thread ID if successful and an error message otherwise.
async fn create_thread(ip: Option<SecureClientIp>, msg: String) -> impl IntoResponse {
    // If IP extraction failed, log error (points to error in proxy configuration) and return. Otherwise, create thread.
    let result = match ip {
        None => {
            if crate::CONFIG.msg_ignore_ip {
                contact::create_thread(std::net::IpAddr::from([0, 0, 0, 0]), msg).await
            } else {
                error!("Failed to extract IP, is proxy configured with X-Forwarded-For header?");
                Err(MessageSendError::DatabaseError)
            }
        }
        Some(ip) => contact::create_thread(ip.0, msg).await,
    };

    // Return newly created thread's ID or map error to response/status code
    match result {
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
        );
    };

    // Send message and report result
    match crate::contact::send_message(thread, msg).await {
        Ok(()) => (StatusCode::OK, String::new()),
        Err(e) => ((&e).into(), format!("Error sending message: {e}")),
    }
}

/// Handles a GET request for the messages in a thread.
async fn get_messages(Path(thread): Path<String>) -> impl IntoResponse {
    // Parse thread ID
    let Ok(thread) = thread.parse::<ThreadId>() else {
        return (
            StatusCode::BAD_REQUEST,
            "Error: ill-formed thread ID (should be a 64-bit hexadecimal integer)".to_string(),
        )
            .into_response();
    };

    // Get messages and return
    match contact::get_messages(thread).await {
        Ok(msgs) => (StatusCode::OK, Json(msgs)).into_response(),
        Err(e) => (StatusCode::from(&e), format!("Error: {e}")).into_response(),
    }
}

impl From<&MessageSendError> for StatusCode {
    fn from(err: &MessageSendError) -> Self {
        match err {
            MessageSendError::DatabaseError => StatusCode::INTERNAL_SERVER_ERROR,
            MessageSendError::TooLong => StatusCode::PAYLOAD_TOO_LARGE,
            MessageSendError::ThreadFull => StatusCode::TOO_MANY_REQUESTS,
            MessageSendError::InboxFull => StatusCode::SERVICE_UNAVAILABLE,
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
