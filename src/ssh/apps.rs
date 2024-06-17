use std::sync::Arc;

use color_eyre::Result;

use tracing::debug;

use super::{
    content::{File, SshContent},
    session::SshSession,
    terminal::TerminalUtils,
};

/// A trait providing functionality for a running app (state machine), including the ability
/// to receive a byte of data and startup functionality.
pub trait RunningApp: Send {
    /// Starts the app, returning the initial state along with some initial reponse data
    /// (basically a starting render) on sucess. On failure, returns a response to send
    /// as if as a normal command (e.g. "file not found").
    fn startup(
        session: &SshSession,
        command: String,
    ) -> Result<(Box<dyn RunningApp>, Vec<u8>), Vec<u8>>
    where
        Self: Sized;
    /// Processes one byte of data from the user input, returning the response.
    fn data(&mut self, data: u8) -> Vec<u8>;
    /// Processes a resize request from the client, returning the response.
    fn resize(&mut self, width: u32, height: u32) -> Vec<u8>;
}

/// The state of a running instance of vim.
pub struct Vim<'a> {
    /// The content of the ssh server, kept to ensure that `self.file` stays alive.
    _ssh_content: Arc<SshContent>,
    /// Current cursor position (x,y), where (0,0) is the top left of the file.
    cursor_pos: (usize, usize),
    /// Current scroll position (line, subline), giving the line at the top of the screen and
    /// (if we've scrolled horizontally through a wrapping line) how many screen-widths of the
    /// line we've scrolled past already.
    scroll_pos: (usize, usize),
    /// The current size of the terminal, in characters.
    term_size: (u16, u16),
    /// The available height for the file (not including bottom text, usually term_size.1 - 1).
    available_height: usize,
    /// The file we are currently viewing.
    file: &'a File,
}
impl<'a> Vim<'a> {
    /// Helper method to clear and rerender the file, returning the necessary response to do so.
    ///
    /// Assumes that `cursor_pos` is onscreen for current `scroll_pos`.
    fn render(&self) -> Vec<u8> {
        // Clear the screen and move the cursor
        let mut response = TerminalUtils::new().clear().move_cursor(0, 0).into_data();

        // Output the file's contents, beginning at the scrolled location.
        // `lines` iterates through the file.
        // `current_line` holds the line of the file we're processing now.
        // `current_line_char` specifies where in the file's line this screen's line starts.
        let mut lines = self.file.lines.iter().skip(self.scroll_pos.0);
        let mut current_line = lines.next();
        let mut current_line_start = self.scroll_pos.1 * self.term_size.0 as usize;
        for y in 0..self.available_height {
            response.append(&mut TerminalUtils::new().move_cursor(0, y as u16).into_data());
            match current_line {
                None => {
                    // The file is over, print placeholder
                    response.push(b'~');
                }
                Some(line) if line.len() >= current_line_start + self.term_size.0 as usize => {
                    // Our current line will wrap, just print what we can and update `current_line_start`
                    response.extend(
                        line[current_line_start..current_line_start + self.term_size.0 as usize]
                            .as_bytes(),
                    );
                    current_line_start += self.term_size.0 as usize;
                }
                Some(line) => {
                    // Our current line will fit on this line, so print the rest of it and step forward
                    response.extend(line[current_line_start..].as_bytes());
                    current_line = lines.next();
                    current_line_start = 0;
                }
            }
        }
        response.extend(b"\r\n: Ctrl-C to quit");

        // Reset the cursor, first finding screen coordinates. We assume that the current scroll is valid,
        // so we cast using `as`. If the coordinates are out of bounds, this is a bug / unconsidered edge case
        // where there are no valid coordinates, but `as` won't crash, just behave strangely.
        let (screen_x, screen_y) = self.get_cursor_screen();
        response.append(
            &mut TerminalUtils::new()
                .move_cursor(screen_x as u16, screen_y as u16)
                .into_data(),
        );

        response
    }

    /// Moves the cursor to the position in `cursor_pos`, scrolling and rerendering if necessary.
    fn update_cursor(&mut self) -> Vec<u8> {
        // Adjust to screen coordinates, and rescroll if they don't fit
        let mut must_rerender = false;
        let (screen_x, screen_y) = loop {
            let (screen_x, screen_y) = self.get_cursor_screen();
            if screen_y < 0 {
                // Must scroll up, first by subline then by line
                if self.scroll_pos.1 > 0 {
                    self.scroll_pos.1 -= 1;
                } else {
                    self.scroll_pos.0 -= 1;
                }
            } else if screen_y >= self.available_height as isize {
                // Must scroll down, by subline if possible (requires enough room in line)
                if (self.scroll_pos.1 + 1) * (self.term_size.0 as usize)
                    < self.file.lines[self.scroll_pos.0].len()
                {
                    self.scroll_pos.1 += 1;
                } else {
                    self.scroll_pos.1 = 0;
                    self.scroll_pos.0 += 1;
                }
            } else {
                break (screen_x as u16, screen_y as u16);
            }
            must_rerender = true;
        };

        // Render the new cursor, doing a full rerender if we scrolled.
        if must_rerender {
            self.render()
        } else {
            TerminalUtils::new()
                .move_cursor(screen_x, screen_y)
                .into_data()
        }
    }

    /// Helper to get the screen position of the cursor from the current `cursor_pos`, `scroll_pos`, and `term_size`.
    /// If this returns an out-of-bounds point, scrolling should be adjusted.
    fn get_cursor_screen(&self) -> (isize, isize) {
        // If cursor is behind first line of screen, or on it but left of scroll_pos, are above screen, so return (0, -1)
        if self.cursor_pos.1 < self.scroll_pos.0
            || self.cursor_pos.1 == self.scroll_pos.0
                && self.cursor_pos.0 < self.scroll_pos.1 * self.term_size.0 as usize
        {
            return (0, -1);
        }
        // Find the first line onscreen, and count how many lines until we get to the current line, including wrapping.
        // Start at `-self.scroll_pos.1` because first line may start above screen.
        let mut screen_y = -(self.scroll_pos.1 as isize);
        for line in self
            .file
            .lines
            .iter()
            .skip(self.scroll_pos.0)
            .take(self.cursor_pos.1 - self.scroll_pos.0)
        {
            screen_y += line.len() as isize / self.term_size.0 as isize + 1;
        }
        // Find the effective x position, snapping back to the end of short lines
        let effective_x =
            self.cursor_pos
                .0
                .min(self.file.lines[self.cursor_pos.1].len().max(1) - 1) as isize;
        // If effective x position is off screen, we will wrap, so adjust y and reduce x accordingly
        screen_y += effective_x / self.term_size.0 as isize;
        (effective_x % self.term_size.0 as isize, screen_y)
    }
}
impl<'a> RunningApp for Vim<'a> {
    fn startup(
        session: &SshSession,
        command: String,
    ) -> Result<(Box<(dyn RunningApp + 'static)>, Vec<u8>), Vec<u8>> {
        let content = session.content.clone();
        let full_path = command
            .split(' ')
            .nth(1)
            .ok_or_else(|| Vec::from(b"vi: usage: vi <filename>\r\n" as &[u8]))?;
        let file = content
            .get_file(session.current_dir, full_path)
            .ok_or_else(|| format!("vi: cannot open \"{}\": No such file\r\n", full_path))?;
        let vim = Vim {
            _ssh_content: Arc::clone(&content),
            cursor_pos: (0, 0),
            scroll_pos: (0, 0),
            term_size: (
                session.term_size.0.try_into().unwrap(),
                session.term_size.1.try_into().unwrap(),
            ),
            available_height: session.term_size.1 as usize - 1,
            // SAFETY: `file` references `content`, which is guarenteed to live as long as this `Vim` object due to the `_ssh_content` reference
            file: unsafe { &*(file as *const File) },
        };
        let response = vim.render();
        Ok((Box::new(vim), response))
    }
    fn data(&mut self, data: u8) -> Vec<u8> {
        match data {
            b'h'..=b'l' => {
                enum Movement {
                    X(isize),
                    Y(isize),
                }
                // Cursor movement
                let movement = match data {
                    b'h' => Movement::X(-1),
                    b'j' => Movement::Y(1),
                    b'k' => Movement::Y(-1),
                    b'l' => Movement::X(1),
                    _ => unreachable!(),
                };
                match movement {
                    Movement::X(delta) => {
                        // Horizontal movement is a little complex due to beyond line end possibility.
                        let last_char = self.file.lines[self.cursor_pos.1].len().max(1) - 1;
                        if self.cursor_pos.0 >= last_char {
                            // If we're at or beyond end, moving right is no-op. Moving left puts us on last character of line prior to executing move normally.
                            if delta < 0 {
                                self.cursor_pos.0 = last_char;
                            } else {
                                return vec![];
                            }
                        }
                        // Get the new x coordinate, being careful not to overflow left
                        let new_x = (self.cursor_pos.0 as isize + delta).max(0) as usize;
                        self.cursor_pos.0 = new_x;
                    }
                    Movement::Y(delta) => {
                        // Get the new coordinates, clamped to the file's range.
                        let new_y = (self.cursor_pos.1 as isize + delta as isize)
                            .clamp(0, self.file.lines.len() as isize - 1)
                            as usize;
                        self.cursor_pos.1 = new_y;
                    }
                }
                // Update the cursor
                self.update_cursor()
            }
            b'$' => {
                // Move to end of line by setting cursor x to high value (not too high to avoid isize overflow)
                self.cursor_pos.0 = usize::MAX / 4;
                self.update_cursor()
            }
            _ => {
                debug!("data '{data:?}' not implemented for vim");
                vec![]
            }
        }
    }
    fn resize(&mut self, width: u32, height: u32) -> Vec<u8> {
        self.term_size = (width as u16, height as u16);
        self.available_height = height as usize - 1;

        // If cursor is off screen, scroll to it
        let mut result = self.update_cursor();
        result.extend(self.render());
        result
    }
}
