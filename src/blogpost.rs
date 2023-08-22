use chrono::NaiveDateTime;
use color_eyre::eyre::eyre;
use color_eyre::Result;
use markdown::mdast::Node;
use serde::{Deserialize, Serialize};

/// One blog post and all of its content and metadata.
#[derive(Serialize, Deserialize, Clone, Debug)]
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
#[derive(Serialize, Clone, Debug)]
pub struct BlogPostContent {
    pub markdown: String,
    pub markdown_ast: Node,
}

impl BlogPostContent {
    fn new(markdown: String) -> Result<Self> {
        let markdown_ast = markdown::to_mdast(&markdown, &markdown::ParseOptions::gfm())
            .map_err(|e| eyre!("{}", e))?;

        // Find all code block children
        let all_descendants = all_descendants(&markdown_ast);
        let code_blocks: Vec<_> = all_descendants
            .iter()
            .filter_map(|node| match node {
                Node::Code(ref c) => Some(c),
                _ => None,
            })
            .collect();

        for code_block in code_blocks {
            dbg!(code_block);
            dbg!(Self::convert_code(code_block.value.as_str()));
        }

        Ok(BlogPostContent {
            markdown,
            markdown_ast,
        })
    }

    fn convert_code(code: &str) -> Result<String> {
        use syntect::html::{ClassStyle, ClassedHTMLGenerator};
        use syntect::parsing::SyntaxSet;
        use syntect::util::LinesWithEndings;

        let syntax_set = SyntaxSet::load_defaults_newlines();
        let syntax = syntax_set.find_syntax_by_name("R").unwrap();
        let mut html_generator =
            ClassedHTMLGenerator::new_with_class_style(syntax, &syntax_set, ClassStyle::Spaced);
        for line in LinesWithEndings::from(code) {
            html_generator.parse_html_for_line_which_includes_newline(line)?;
        }
        Ok(html_generator.finalize())
    }
}

/// Utility to get all descendants of a node.
fn all_descendants(node: &Node) -> Vec<&Node> {
    // Setup
    let mut all_nodes = vec![node];
    let mut to_visit = vec![node];
    /// Macro to do extension of vectors for shortening code
    macro_rules! e {
        ($node:expr) => {{
            all_nodes.extend($node.children.iter());
            to_visit.extend($node.children.iter());
        }};
    }
    // Traversal
    while let Some(node) = to_visit.pop() {
        match node {
            Node::Root(x) => e!(x),
            Node::BlockQuote(x) => e!(x),
            Node::FootnoteDefinition(x) => e!(x),
            Node::MdxJsxFlowElement(x) => e!(x),
            Node::List(x) => e!(x),
            Node::Delete(x) => e!(x),
            Node::Emphasis(x) => e!(x),
            Node::MdxJsxTextElement(x) => e!(x),
            Node::Link(x) => e!(x),
            Node::LinkReference(x) => e!(x),
            Node::Strong(x) => e!(x),
            Node::Heading(x) => e!(x),
            Node::Table(x) => e!(x),
            Node::TableRow(x) => e!(x),
            Node::TableCell(x) => e!(x),
            Node::ListItem(x) => e!(x),
            Node::Paragraph(x) => e!(x),
            _ => {} // No children (hopefully)
        }
    }
    all_nodes
}
