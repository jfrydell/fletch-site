use std::net::SocketAddr;

use crate::contact::ThreadId;

/// Handles the `msg` command, returning the output to be sent to the user's terminal.
pub async fn msg(command: &str, ip: SocketAddr) -> Vec<u8> {
    // Split arguments and dispatch to correct handler
    let mut args = command.split(' ');
    args.next(); // command name
    let a1 = args.next();
    let a2 = args.next();
    let mut response = match (a1, a2) {
        (Some("send"), _) => msg_send(command, ip).await,
        (Some("reply"), Some(thread_id)) => msg_reply(thread_id, command).await,
        (Some("view"), Some(thread_id)) => msg_view(thread_id).await,
        _ => msg_usage(),
    };

    // Add newline to response and carriage returns before each newline
    response.push_str("\n\n");
    response.replace('\n', "\r\n").into_bytes()
}

async fn msg_send(command: &str, ip: SocketAddr) -> String {
    // Get message to send (splice off first two arguments) and check size lower bound
    let msg = command.splitn(3, ' ').nth(2).unwrap_or_default();
    if msg.len() < 25 {
        return "Usage: `msg send <BODY...>`
Initial message body must be at least 25 characters (mostly to avoid accidentally sending something. Use `msg help` (or just `msg`) to see some usage info.".to_string();
    }

    // Send message, displaying result to user
    match crate::contact::create_thread(ip, msg.to_string()).await {
        Ok(id) => {
            format!("Message sent! Thread ID: {id} (don't lose that if you want a reply!)")
        }
        Err(e) => format!("Error sending message: {e}"),
    }
}

async fn msg_reply(thread_id: &str, command: &str) -> String {
    // Parse thread id
    let Ok(thread_id) = thread_id.parse::<ThreadId>() else {
        return "Error: ill-formed thread ID (should be a 64-bit hexadecimal integer)".to_string();
    };

    // Get message to send (splice off first 3 arguments) and check size lower bound
    let msg = command.splitn(4, ' ').nth(3).unwrap_or_default();
    if msg.len() < 10 {
        return "Usage: `msg reply <THREAD> <BODY...>`
Message body must be at least 10 characters (mostly to avoid accidentally sending something. Use `msg help` (or just `msg`) to see some usage info.".to_string();
    }

    // Send message, displaying result to user
    match crate::contact::send_message(thread_id, msg.to_string()).await {
        Ok(()) => {
            format!("Message sent on thread ID: {thread_id} (don't lose that if you want a reply!)")
        }
        Err(e) => format!("Error sending message: {e}"),
    }
}

async fn msg_view(thread_id: &str) -> String {
    // Parse thread id
    let Ok(thread_id) = thread_id.parse::<ThreadId>() else {
        return "Error: ill-formed thread ID (should be a 64-bit hexadecimal integer)".to_string();
    };

    // Get messages, printing error if necessary
    let messages = match crate::contact::get_messages(thread_id).await {
        Ok(msgs) => msgs,
        Err(e) => return format!("Error loading thread: {e}"),
    };
    let mut result = format!("Thread {thread_id}:\n");
    for message in messages {
        result.push_str(&format!(
            "({}) {} {}\n",
            message.timestamp,
            if message.response { "Me: " } else { "You:" },
            message.contents
        ));
    }
    result
}

fn msg_usage() -> String {
    "Usage: `msg send <BODY...>` or `msg view <THREAD>` or `msg reply <THREAD> <BODY...>`

Have feedback on the site? A comment about a page? Just want to get in touch / send a message?
This command allows you to send a message straight from your terminal to mine (see the project page (TODO) for more).

To send your first message, just use `msg send` followed by any length of message, which will start a new thread and return the corresponding thread ID.
Then, you can use `msg view` along with the thread ID to see your message and, eventually (hopefully), my reply.
If you want to send a follow up to your initial message or a response to mine, you can use `msg reply` with the thread ID and your response.

The thread IDs here are the same as those in my http/html website's contact form (TODO), so you can also view/send messages there.

If you have any questions, well, you should know how to get in touch now! I look forward to hearing from you!".to_string()
}
