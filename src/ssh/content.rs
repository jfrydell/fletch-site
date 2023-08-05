use std::{borrow::Cow, collections::BTreeMap};

pub static WELCOME_MESSAGE: &[u8] = "Welcome to the SSH version of my website! This is very much a work in progress, but I hope you enjoy it nonetheless!\r
To navigate, use the 'ls' and 'cd' commands to see the available pages and 'cat' or 'vi' to view them. Type 'exit' or 'logout' to leave. Type 'help' anytime to see this message.".as_bytes();

/// The rendered content for the SSH server.
#[derive(Debug)]
pub struct SshContent {
    /// The directories of the virtual filesystem, with the root first.
    pub directories: Vec<Directory>,
}
impl SshContent {
    /// Render the SSH content from the given content.
    pub fn new(content: &crate::Content) -> Self {
        // Get an empty content to start
        let mut result = Self {
            directories: vec![Directory {
                path: "/".to_string(),
                ..Default::default()
            }],
        };

        // Add projects directory
        let projects_i = result.add_child(0, "projects".to_string());
        for project in content.projects.iter() {
            result.add_file(
                projects_i,
                format!("{}.txt", project.url),
                File::new(project.to_string().replace('\n', "\r\n")),
            );
        }

        result
    }
    /// Gets the directory at the given index.
    pub fn get(&self, i: usize) -> &Directory {
        &self.directories[i]
    }
    /// Gets the directory at the given path.
    pub fn dir_at(&self, path: &str) -> Option<&Directory> {
        let mut dir = &self.directories[0];
        for part in path.split('/') {
            if part.is_empty() || part == "." {
                continue;
            }
            if part == ".." {
                if let Some(parent) = dir.parent {
                    dir = &self.directories[parent];
                }
                continue;
            }
            dir = match dir.directories.get(part) {
                Some(i) => &self.directories[*i],
                None => return None,
            };
        }
        Some(dir)
    }
    /// Gets the file at the given path, if necessary relative to the given directory.
    pub fn get_file(&self, current_dir: usize, full_path: &str) -> Option<&File> {
        let (path, filename) = match full_path.rsplit_once('/') {
            Some((directory, filename)) => {
                if directory.starts_with('/') || directory.is_empty() {
                    // Absolute path, no need for current path addition
                    (Cow::Borrowed(directory), filename)
                } else {
                    let mut d = self.directories[current_dir].path.clone();
                    d.push_str(directory);
                    (Cow::Owned(d), filename)
                }
            }
            None => (
                Cow::Borrowed(self.directories[current_dir].path.as_str()),
                full_path,
            ),
        };
        self.dir_at(&path).and_then(|d| d.files.get(filename))
    }
    /// Add a child directory to a `Directory` specified by index, returning the index of the child.
    fn add_child(&mut self, parent_i: usize, child_name: String) -> usize {
        let child_i = self.directories.len();
        let parent = &mut self.directories[parent_i];
        let child = Directory {
            path: {
                let mut path = parent.path.clone();
                if parent_i != 0 {
                    path.push('/');
                }
                path.push_str(&child_name);
                path
            },
            parent: Some(parent_i),
            ..Default::default()
        };
        parent.directories.insert(child_name, child_i);
        self.directories.push(child);
        child_i
    }
    /// Add a file to a `Directory` specified by index.
    fn add_file(&mut self, dir_i: usize, filename: String, contents: File) {
        let dir = &mut self.directories[dir_i];
        dir.files.insert(filename, contents);
    }
}

/// A directory in the virtual filesystem, containing a list of files and other directories.
#[derive(Debug, Default)]
pub struct Directory {
    /// The full path to this directory, always ending in a `/`.
    pub path: String,
    /// The parent of this directory (`None` if root).
    pub parent: Option<usize>,
    /// Subdirectories of this directory, indexed by name.
    pub directories: BTreeMap<String, usize>,
    /// Files in this directory, indexed by name.
    pub files: BTreeMap<String, File>,
}

/// A file in the virtual filesystem, containing an array of lines.
#[derive(Debug, Default)]
pub struct File {
    /// The raw contents of the file as a `String`.
    pub contents: String,
    /// The contents of the file, as an array of lines. There is always at least one (possibly-empty) line.
    pub lines: Vec<String>,
}
impl File {
    pub fn new(contents: String) -> Self {
        let lines: Vec<String> = contents.split("\r\n").map(|s| s.to_string()).collect();
        Self { contents, lines }
    }
    pub fn raw_contents(&self) -> &[u8] {
        self.contents.as_bytes()
    }
}
