use std::{collections::HashMap, sync::Mutex};

use chrono::NaiveDateTime;
use color_eyre::eyre::eyre;
use color_eyre::Result;
use markdown::mdast::*;
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
    /// Render the content as markdown, but with a set of callbacks for handling some elements specially.
    /// If multiple callbacks match an element, only the first is used.
    pub fn render_markdown(&self, callbacks: &[RenderCallback]) -> Result<String> {
        let mut rendered = String::new();
        self.render_markdown_recurse(&self.markdown_ast, &mut rendered, callbacks)?;
        Ok(rendered)
    }
    /// Recursive descent for markdown rendering. This is mainly used by `render_markdown`, but can be used
    /// directly to render a subset of the markdown AST (for example, in a callback).
    pub fn render_markdown_recurse(
        &self,
        node: &Node,
        result: &mut String,
        callbacks: &[RenderCallback],
    ) -> Result<()> {
        // Check for callbacks
        for callback in callbacks {
            if let Some(rendered) = callback.render(node, self, || {
                let mut rendered = String::new();
                for child in node.children().unwrap_or(&vec![]) {
                    self.render_markdown_recurse(child, &mut rendered, callbacks)?;
                }
                Ok(rendered)
            })? {
                result.push_str(&rendered);
                return Ok(());
            }
        }

        // Helper macro to render children to result
        macro_rules! children {
            ($node:expr) => {
                for child in $node.children.iter() {
                    self.render_markdown_recurse(child, result, callbacks)?;
                }
            };
        }

        // Render node
        match node {
            Node::Root(r) => children!(r),
            Node::FootnoteDefinition(d) => {
                result.push_str(&format!("[^{}]: ", d.identifier));
                children!(d);
                result.push('\n');
            }
            Node::FootnoteReference(r) => result.push_str(&format!("[^{}]", r.identifier)),
            Node::InlineCode(c) => result.push_str(&format!("`{}`", c.value)),
            Node::Delete(d) => {
                result.push_str("~~");
                children!(d);
                result.push_str("~~");
            }
            Node::Emphasis(e) => {
                result.push_str("*");
                children!(e);
                result.push_str("*");
            }
            Node::Link(l) => {
                result.push('[');
                children!(l);
                result.push_str(&format!("]({})", l.url));
            }
            Node::Strong(s) => {
                result.push_str("**");
                children!(s);
                result.push_str("**");
            }
            Node::Text(t) => result.push_str(&t.value),
            Node::Code(c) => result.push_str(&format!("```\n{}\n```", c.value)),
            Node::Heading(h) => {
                result.push_str(&"#".repeat(h.depth as usize));
                result.push(' ');
                children!(h);
                result.push('\n');
            }
            Node::Paragraph(p) => {
                children!(p);
                result.push('\n');
            }
            _ => {
                return Err(eyre!("Unsupported markdown node: {:?}", node));
            }
        }
        Ok(())
    }

    /// Render the content as HTML, but with a set of callbacks for handling some elements specially.
    /// If multiple callbacks match an element, only the first is used.
    pub fn render_html(&self, callbacks: &[RenderCallback]) -> Result<String> {
        let mut rendered = String::new();
        self.render_html_recurse(&self.markdown_ast, &mut rendered, callbacks)?;
        Ok(rendered)
    }
    /// Recursive descent for HTML rendering. This is mainly used by `render_html`, but can be used
    /// directly to render a subset of the markdown AST (for example, in a callback).
    pub fn render_html_recurse(
        &self,
        node: &Node,
        result: &mut String,
        callbacks: &[RenderCallback],
    ) -> Result<()> {
        // Check for callbacks
        for callback in callbacks {
            if let Some(rendered) = callback.render(node, self, || {
                let mut rendered = String::new();
                for child in node.children().unwrap_or(&vec![]) {
                    self.render_html_recurse(child, &mut rendered, callbacks)?;
                }
                Ok(rendered)
            })? {
                result.push_str(&rendered);
                return Ok(());
            }
        }

        // Helper macro to render children to result
        macro_rules! children {
            ($node:expr) => {
                for child in $node.children.iter() {
                    self.render_html_recurse(child, result, callbacks)?;
                }
            };
        }

        // Render node
        match node {
            Node::Root(r) => children!(r),
            Node::FootnoteDefinition(d) => {
                result.push_str(&format!(
                    "<a id='footnote-{}', href='#footnote-ref-{}'>[{}]</a>",
                    d.identifier, d.identifier, d.identifier
                ));
                children!(d);
            }
            Node::FootnoteReference(r) => result.push_str(&format!(
                "<sup id='footnote-ref-{}'><a href='#footnote-{}'>{}</a></sup>",
                r.identifier, r.identifier, r.identifier
            )),
            Node::InlineCode(c) => result.push_str(&format!("<code>{}</code>", c.value)),
            Node::Delete(d) => {
                result.push_str("<s>");
                children!(d);
                result.push_str("</s>");
            }
            Node::Emphasis(e) => {
                result.push_str("<em>");
                children!(e);
                result.push_str("</em>");
            }
            Node::Link(l) => {
                result.push_str(&format!("<a href='{}'>", l.url));
                children!(l);
                result.push_str("</a>");
            }
            Node::Strong(s) => {
                result.push_str("<strong>");
                children!(s);
                result.push_str("</strong>");
            }
            Node::Text(t) => result.push_str(&t.value),
            Node::Code(c) => result.push_str(&format!("<pre><code>{}</code></pre>", c.value)),
            Node::Heading(h) => {
                result.push_str(&format!("<h{}", h.depth));
                children!(h);
                result.push_str(&format!("</h{}>", h.depth));
            }
            Node::Paragraph(p) => {
                result.push_str("<p>");
                children!(p);
                result.push_str("</p>");
            }

            _ => {
                return Err(eyre!("Unsupported markdown node: {:?}", node));
            }
        }
        Ok(())
    }

    fn new(markdown: String) -> Result<Self> {
        // Parse AST
        let markdown_ast = markdown::to_mdast(&markdown, &markdown::ParseOptions::gfm())
            .map_err(|e| eyre!("{}", e))?;

        // Find all code block children with DFS
        let mut code_blocks = vec![];
        let mut to_visit = vec![&markdown_ast];
        while let Some(node) = to_visit.pop() {
            to_visit.extend(node.children().map(|v| v.iter()).unwrap_or_default());
            if let Node::Code(ref c) = node {
                code_blocks.push(c);
            }
        }

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

/// A callback to alter the rendering of a specific node type.
/// The callback is passed the node ref, the rendered content of the node's children, and any extra
/// node-specific information (like syntax highlighting for code blocks).
pub enum RenderCallback<'a> {
    /// A callback for code blocks, with syntax highlighting info included.
    Code(&'a dyn Fn(&Code, String, &HighlightedCode) -> String),
    /// A callback for links.
    Link(&'a dyn Fn(&Link, String) -> String),
}
impl<'a> RenderCallback<'a> {
    /// Attempts to render a node with the callback, returning `None` if the callback doesn't handle the node.
    /// The `content` is needed for extra information (like syntax highlighting), and the `rendered_children` is
    /// used to get child content for the callback, if necessary.
    fn render(
        &self,
        node: &Node,
        content: &BlogPostContent,
        rendered_children: impl Fn() -> Result<String>,
    ) -> Result<Option<String>> {
        Ok(match self {
            RenderCallback::Code(f) => match node {
                Node::Code(c) => Some(f(
                    c,
                    rendered_children()?,
                    content
                        .code_highlights
                        .get(&c.position.as_ref().unwrap().start.offset)
                        .ok_or_else(|| eyre!("Code block has no position info"))?,
                )),

                _ => None,
            },
            RenderCallback::Link(f) => match node {
                Node::Link(l) => Some(f(l, rendered_children()?)),
                _ => None,
            },
        })
    }
}
