/// A virtual shell implementing line discipline, echoing, and backspace, receiving individual character inputs and passing output back to the client.
#[derive(Debug)]
pub struct Shell {
    /// The current cursor position
    cursor: usize,
    /// Command history
    history: Vec<String>,
    /// Current command history, as explored by up/down arrow. First element is the "current" (newest) line.
    ///
    /// This history is not saved between command runs, but is duplicated so the user can edit past commands without affecting the real history.
    current_history: Vec<String>,
    /// Index into the current_history specifying what we're editing now
    history_index: usize,
    /// Escape sequence buffer
    escape: Vec<u8>,
}
impl Default for Shell {
    fn default() -> Self {
        Self {
            cursor: 0,
            history: vec![],
            current_history: vec![String::new()],
            history_index: 0,
            escape: vec![],
        }
    }
}

impl Shell {
    /// Processes a byte of input, returning a response to send back as well as optionally a command to run.
    /// If the command is "", no command is run, but the prompt is resent.
    ///
    /// (Some logic taken from [https://github.com/offirgolan/Shell/blob/master/read-line.c])
    pub fn process(&mut self, data: u8) -> (Vec<u8>, Option<String>) {
        if !self.escape.is_empty() {
            return (self.process_escape(data), None);
        }
        let line = {
            self.get_line();
            self.current_history.get_mut(self.history_index).unwrap()
        };
        match data {
            13 | 10 => {
                // Newline, echo and run command
                let command = std::mem::take(line);
                self.history.push(command.clone());
                // Reset current history/command
                self.current_history = vec![String::new()];
                self.history_index = 0;
                self.cursor = 0;
                (vec![13, 10], Some(command))
            }
            8 | 127 => {
                // Backspace, remove character from buffer and overwrite as necessary
                if self.cursor > 0 {
                    line.remove(self.cursor - 1);
                    self.cursor -= 1;
                    if self.cursor == line.len() {
                        // At end of line, so go back, overwrite with space, go back again
                        (vec![8, 32, 8], None)
                    } else {
                        // Middle of line, so go back, overwrite with rest of line, go back to original location
                        let mut response = vec![8];
                        response.extend(line[self.cursor..].bytes());
                        response.push(32); // Overwrite last character (since new line has one fewer than old)
                        response.extend(std::iter::repeat(8).take(line.len() - self.cursor + 1));
                        (response, None)
                    }
                } else {
                    (vec![], None)
                }
            }
            3 => {
                // CTRL-C, clear line and reset without running command
                // Reset current history/command
                self.current_history = vec![String::new()];
                self.history_index = 0;
                self.cursor = 0;
                // Send newline and empty command (for prompt)
                (vec![13, 10], Some(String::new()))
            }
            4 => {
                // CTRL-D, close session
                (vec![], Some("exit".to_string()))
            }
            1 => {
                // CTRL-A, move cursor to start of line
                let response = vec![8; self.cursor];
                self.cursor = 0;
                (response, None)
            }
            5 => {
                // CTRL-E, move cursor to end of line
                let response = vec![8; line.len() - self.cursor];
                self.cursor = line.len();
                (response, None)
            }
            27 => {
                // Escape sequence, wait for next two bytes
                self.escape = vec![27];
                (vec![], None)
            }
            32.. => {
                // Normal character, insert and echo
                line.insert(self.cursor, data as char);
                self.cursor += 1;
                if self.cursor < line.len() {
                    // Inserted in the middle, send [inserted, rest of line, move cursor back]
                    let mut response = vec![];
                    response.push(data);
                    response.extend(line[self.cursor..].bytes());
                    response.extend(vec![8; line.len() - self.cursor]);
                    (response, None)
                } else {
                    // Inserted at the end, send [inserted]
                    (vec![data], None)
                }
            }
            _ => (vec![], None),
        }
    }

    /// Processes a byte of data while in the middle of an escape sequence
    fn process_escape(&mut self, data: u8) -> Vec<u8> {
        self.escape.push(data);
        if self.escape.len() == 3 {
            // Escape sequence complete
            // Get current line, updating histories if necessary
            let line = {
                self.get_line();
                self.current_history.get(self.history_index).unwrap()
            };
            let escape = std::mem::take(&mut self.escape);
            match escape.as_slice() {
                [27, 91, 68] => {
                    // Left arrow, move cursor back
                    if self.cursor > 0 {
                        self.cursor -= 1;
                        vec![27, 91, 68]
                    } else {
                        vec![]
                    }
                }
                [27, 91, 67] => {
                    // Right arrow, move cursor forward
                    if self.cursor < line.len() {
                        self.cursor += 1;
                        vec![27, 91, 67]
                    } else {
                        vec![]
                    }
                }
                [27, 91, 65] | [27, 91, 66] => {
                    // Up or down arrow, move in history
                    if escape[2] == 65 {
                        // Up arrow
                        self.history_index += 1;
                    } else {
                        // Down arrow, return if already at current
                        if self.history_index == 0 {
                            return vec![];
                        }
                        self.history_index -= 1;
                    }
                    // Clear current line
                    let mut response = vec![8; self.cursor];
                    response.extend(std::iter::repeat(32).take(line.len()));
                    response.extend(std::iter::repeat(8).take(line.len()));
                    // Get new line
                    let line = {
                        self.get_line();
                        self.current_history.get(self.history_index).unwrap()
                    };
                    // Write new line and update cursor
                    self.cursor = line.len();
                    response.extend(line.bytes());
                    response
                }
                _ => vec![],
            }
        } else {
            vec![]
        }
    }

    /// Get the current line of input, updating the current history and clamping the history index if necessary.
    /// Rather then returning a reference, we just update `self.current_history` and `self.history_index` to avoid borrowing issues.
    fn get_line(&mut self) {
        // Clamp history index so we don't go too far back
        if self.history_index > self.history.len() {
            self.history_index = self.history.len();
        }
        // Extend current history to reach history index
        while self.current_history.len() <= self.history_index {
            self.current_history
                .push(self.history[self.history.len() - self.current_history.len()].clone())
        }
    }
}

/// Some utilities for fancy terminal output.
///
/// ## Example
/// ```
/// let buffer: Vec<u8> = TerminalUtils::new(80, 24).place(40, 12).into_data();
/// ```
#[allow(unused)]
#[derive(Clone)]
pub struct TerminalUtils {
    pos: Option<(u16, u16)>,
    data: Vec<u8>,
}

#[allow(unused)]
impl TerminalUtils {
    /// Creates a new terminal utility for the given width and height.
    pub fn new() -> Self {
        Self {
            pos: None,
            data: vec![],
        }
    }

    /// Places a character `c` at a location (x,y).
    pub fn place(mut self, x: u16, y: u16, c: u8) -> Self {
        // Move cursor to new position if necessary, and update it
        if self.pos != Some((x, y)) {
            self = self.move_cursor(x, y);
        }
        self.pos = Some((x + 1, y));

        // Write symbol to queue of data to be sent
        self.data.push(c);
        self
    }
    /// Hides the cursor.
    pub fn hide_cursor(mut self) -> Self {
        self.data.extend(b"\x1b[?25l");
        self
    }
    /// Shows the cursor.
    pub fn show_cursor(mut self) -> Self {
        self.data.extend(b"\x1b[?25h");
        self
    }
    /// Moves the cursor.
    pub fn move_cursor(mut self, x: u16, y: u16) -> Self {
        self.data
            .extend(format!("\x1b[{};{}H", y + 1, x + 1).as_bytes());
        self
    }
    /// Clears the screen (doesn't move cursor).
    pub fn clear(mut self) -> Self {
        self.data.extend(b"\x1b[2J");
        self
    }

    /// Gets the data for all the operations done.
    pub fn into_data(self) -> Vec<u8> {
        self.data
    }
}
