use std::{borrow::Cow, collections::BTreeMap};

use color_eyre::{eyre::eyre, Result};

pub static WELCOME_MESSAGE: &[u8] = "Welcome to the SSH version of my website! This is very much a work in progress, but I hope you enjoy it nonetheless!\r
To navigate, use the 'ls' and 'cd' commands to see the available pages and 'cat' or 'vi' to view them.\r
If you have any feedback, use the 'msg' command to send it (or view any replies to messages you've sent).\r
To see this message again, just use `help`, and when you're ready to go, type 'exit' or 'logout' (or Ctrl-D).\r\n".as_bytes();

/// The rendered content for the SSH server.
#[derive(Debug)]
pub struct SshContent {
    /// The directories of the virtual filesystem, with the root first.
    pub directories: Vec<Directory>,
}
impl SshContent {
    /// Render the SSH content from the given content.
    pub fn new(content: &crate::Content) -> Result<Self> {
        // Get an empty content to start
        let mut result = Self {
            directories: vec![Directory {
                path: "/".to_string(),
                ..Default::default()
            }],
        };

        // Add home page and themes page
        result.add_file(
            0,
            "home.txt".to_string(),
            File::new(get_home_page(content)?),
        );
        result.add_file(
            0,
            "themes.txt".to_string(),
            File::new(get_themes_page(content)?),
        );

        // Add projects directory
        let projects_i = result.add_child(0, "projects".to_string());
        for project in content.projects.iter() {
            result.add_file(
                projects_i,
                format!("{}.txt", project.url),
                File::new(project.to_string().replace('\n', "\r\n")),
            );
        }

        // Add blog directory
        let blog_i = result.add_child(0, "blog".to_string());
        for post in content.blog_posts.iter() {
            result.add_file(
                blog_i,
                format!("{}_{}.txt", post.date.date().format("%Y%m%d"), post.url),
                File::new(post.to_string().replace('\n', "\r\n")),
            );
        }

        Ok(result)
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

macro_rules! access_json {
    ($content:expr, $path:expr) => {
        $content
            .get($path)
            .and_then(|v| v.as_str())
            .ok_or_else(|| eyre!("No {} found in {}", stringify!($path), stringify!($content)))?
    };
}

/// Get the home page for the SSH server.
fn get_home_page(content: &crate::Content) -> Result<String> {
    let ascii_art = "
FFFFFFFFFFFFFFF     LLLL               EEEEEEEEEEEEEEE     TTTTTTTTTTTTTT     CCCCCCCCCCCCCC     HHHH       HHHH
FFFFFFFFFFFFFFF     LLLL               EEEEEEEEEEEEEEE     TTTTTTTTTTTTTT     CCCCCCCCCCCCCC     HHHH       HHHH
FFFF                LLLL               EEEE                     TTTT          CCCC               HHHH       HHHH
FFFF                LLLL               EEEE                     TTTT          CCCC               HHHH       HHHH
FFFFFFFFFFFFF       LLLL               EEEEEEEEEEEEEEE          TTTT          CCCC               HHHHHHHHHHHHHHH
FFFFFFFFFFFFF       LLLL               EEEEEEEEEEEEEEE          TTTT          CCCC               HHHHHHHHHHHHHHH
FFFF                LLLL               EEEE                     TTTT          CCCC               HHHH       HHHH
FFFF                LLLL               EEEE                     TTTT          CCCC               HHHH       HHHH
FFFF                LLLLLLLLLLLLLL     EEEEEEEEEEEEEEE          TTTT          CCCCCCCCCCCCCC     HHHH       HHHH
FFFF                LLLLLLLLLLLLLL     EEEEEEEEEEEEEEE          TTTT          CCCCCCCCCCCCCC     HHHH       HHHH
    ";
    let intro_blurb = "RYDELL. There, it's done. Manuallying typeing ASCII art is a pain, and what's the point if it probably won't even fit on your stupid 80x24 terminal?
Anyway, welcome to my website! Congratulations for SSHing into my server and opening this page (and even more congratulations to me for making this work in real life!).
The goal of this SSH-based website to be another version of the HTTP-based site, accessible from the command line. So, all the content should be the exact same across both sites, just with different presentation.
Obviously, that's not completely true (see this text), but that's the idea. So, some things may seem strange, but it's all in the name of including all content from the HTTP site. When it's particularly confusing, I'll leave a note at the top of the file reminding you what function the content serves on the HTTP site.

In this case, the entire idea of the home page is to be a landing page for the site with links to all the other pages, which doesn't translate well to SSH version. So, I've replaced all the links and lists of links with \"SEE FILESYSTEM\", because that's the only way to navigate the site. It's more fun to explore that way!
So, go ahead and explore the site! You can find the home page content below, but as mentioned, it's not much use in this version:";
    let homepage_content = format!(
        "# Fletch Rydell\n*{}*\n\n## About me\n{}\n\n## Projects\n{}\nSEE FILESYSTEM\n\n",
        access_json!(content.index_info, "subtitle"),
        access_json!(content.index_info, "about_me"),
        access_json!(content.index_info, "projects_caption")
    );
    Ok(format!("{}\n{}\n\n{}", ascii_art, intro_blurb, homepage_content).replace('\n', "\r\n"))
}

/// Get the themes page for the SSH server.
fn get_themes_page(content: &crate::Content) -> Result<String> {
    let disclaimer = "NOTE: This page is about the themes available on the HTTP version of the site. There are no themes here, but this page is still here to uphold my promise of including all content from the HTTP site.";
    let mut result = format!(
        "{disclaimer}\n\n# Themes\n## About Themes\n{}\n\n",
        access_json!(content.themes_info, "about_text"),
    );
    for theme in content
        .themes_info
        .get("themes")
        .and_then(|x| x.as_array())
        .ok_or_else(|| eyre!("No themes found in themes_info"))?
    {
        result.push_str(&format!(
            "## {}\n{}\n\n",
            access_json!(theme, "name"),
            access_json!(theme, "description")
        ));
    }
    Ok(result.replace('\n', "\r\n"))
}
