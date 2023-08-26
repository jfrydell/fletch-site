use std::{collections::HashMap, sync::Mutex};

use chrono::NaiveDateTime;
use color_eyre::eyre::eyre;
use color_eyre::Result;
use markdown::mdast::Node;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use tracing::error;

/// One blog post and all of its content and metadata.
#[derive(Serialize, Deserialize, Debug)]
pub struct BlogPost {
    pub title: String,
    pub url: String,
    pub date: NaiveDateTime,

    #[serde(deserialize_with = "deserialize_content")]
    pub content: BlogPostContent,
}

/// Deserialization for blog post content from a string.
fn deserialize_content<'de, D>(de: D) -> Result<BlogPostContent, D::Error>
where
    D: serde::Deserializer<'de>,
{
    String::deserialize(de).and_then(|markdown| {
        BlogPostContent::new(markdown).map_err(|e| serde::de::Error::custom(e.to_string()))
    })
}

/// A blog post's content. This includes the original markdown, the parsed markdown AST, and extra info like code block syntax highlighting.
#[derive(Serialize, Deserialize, Debug)]
pub struct BlogPostContent {
    /// The raw markdown for the post.
    pub markdown: String,
    /// The AST generated from the markdown.
    pub markdown_ast: Node,
    /// A map from positions (`offset` in AST Node start) to syntax highlighting information for Code blocks.
    pub code_highlights: HashMap<usize, HighlightedCode>,
}

impl BlogPostContent {
    fn new(markdown: String) -> Result<Self> {
        // Parse AST
        let markdown_ast = markdown::to_mdast(&markdown, &markdown::ParseOptions::gfm())
            .map_err(|e| eyre!("{}", e))?;

        // Find all code block children and do syntax highlighting
        let all_descendants = all_descendants(&markdown_ast);
        let code_blocks: Vec<_> = all_descendants
            .iter()
            .filter_map(|node| match node {
                Node::Code(ref c) => Some(c),
                _ => None,
            })
            .collect();

        // Generate code highlights map
        let mut code_highlights = HashMap::new();
        for code_block in code_blocks {
            let Some(offset) = code_block.position.as_ref().map(|x| x.start.offset) else {
                return Err(eyre!("Reached code block with no position info"));
            };
            code_highlights.insert(
                offset,
                Self::convert_code(code_block.value.as_str(), &code_block.lang)?,
            );
        }

        Ok(BlogPostContent {
            markdown,
            markdown_ast,
            code_highlights,
        })
    }

    fn convert_code(code: &str, lang: &Option<String>) -> Result<HighlightedCode> {
        use inkjet::{tree_sitter_highlight::HighlightEvent as TSEvt, Highlighter, Language};
        // See if language is supported
        let lang = match lang {
            None => {
                return Err(eyre!("Code block didn't include a language specifier. Use `plain` if no highlighting is desired."));
            }
            Some(lang) => {
                if lang == "plain" {
                    return Ok(HighlightedCode {
                        content: vec![(code.to_string(), None)],
                    });
                }
                Language::from_token(lang)
                    .ok_or_else(|| eyre!("Language {} not supported by inkjet", lang))?
            }
        };
        // Construct highlighter lazily and highlight events
        static HIGHLIGHTER: Lazy<Mutex<Highlighter>> = Lazy::new(|| Mutex::new(Highlighter::new()));
        let mut highlighter = HIGHLIGHTER.lock().unwrap();
        let events = highlighter.highlight_raw(lang, code)?;

        // Build content from event iter
        let mut content = Vec::new();
        let mut current_highlight = None;
        for event in events {
            match event? {
                TSEvt::Source { start, end } => {
                    content.push((code[start..end].to_string(), current_highlight))
                }
                TSEvt::HighlightStart(highlight) => {
                    if current_highlight.is_some() {
                        error!("Nested highlight detected, this is not supported");
                    }
                    current_highlight = Some(highlight.0);
                }
                TSEvt::HighlightEnd => current_highlight = None,
            }
        }

        Ok(HighlightedCode { content })
    }
}

// For use with highlight color indices in `HighlightedCode`s
pub use inkjet::constants::HIGHLIGHT_NAMES;

#[derive(Debug, Serialize, Deserialize)]
pub struct HighlightedCode {
    /// A list of contents, each tagged with a highlight color (from tree-sitter via inkjet).
    pub content: Vec<(String, Option<usize>)>,
}

/// Utility to get all descendants of a markdown node.
fn all_descendants(node: &Node) -> Vec<&Node> {
    // Setup
    let mut all_nodes = vec![node];
    let mut to_visit = vec![node];
    // Traversal
    while let Some(node) = to_visit.pop() {
        all_nodes.extend(node.children().map(|v| v.iter()).unwrap_or_default());
    }
    all_nodes
}
