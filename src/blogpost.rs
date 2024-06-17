use std::{collections::HashMap, iter::Peekable};

use chrono::NaiveDateTime;
use color_eyre::{eyre::bail, Result};
use serde::{Deserialize, Serialize};

/// One blog post and all of its content and metadata.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct BlogPost {
    pub title: String,
    pub url: String,
    pub date: NaiveDateTime,
    pub visibility: i32,
    #[serde(default, rename = "tag")]
    pub tags: Vec<Tag>,

    #[serde(deserialize_with = "deserialize_content")]
    pub content: Content,
}

/// Possible tags for a blog post.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "lowercase", tag = "$text")]
pub enum Tag {
    /// A notes post, with a disclaimer at the top.
    Note,
}

/// The content of a blog post, consisting of (for now) only the `Element`s that make it up.
#[derive(Serialize, Debug, Clone)]
pub struct Content {
    /// The body of the content, consisting of several `Element`s.
    content: Vec<Element>,
    /// Any footnotes, each containing a tag for cross-referencing and some content.
    ///
    /// Should be numbered in order, starting at 1.
    footnotes: Vec<(String, Vec<Element>)>,
}

/// Deserialization for blog post content from a string.
fn deserialize_content<'de, D>(de: D) -> Result<Content, D::Error>
where
    D: serde::Deserializer<'de>,
{
    // Parse the XML body into a String as-is, then parse as jdot event stream.
    let raw = String::deserialize(de)?;
    let mut events = jotdown::Parser::new(&raw).peekable();
    let mut elements = Element::parse_many(&mut events)
        .map_err(|e| serde::de::Error::custom(format!("error deserializing post content: {e}")))?;

    // Number & footnotes
    let footnotes = extract_footnotes(&mut elements)
        .map_err(|e| serde::de::Error::custom(format!("error generating footnotes: {e}")))?;

    Ok(Content {
        content: elements,
        footnotes,
    })
}

// Utility macro to check ending event matches the given container, bailing if not.
macro_rules! assert_container_end {
    ($e:expr, $c:pat) => {
        let e = $e.next();
        match e {
            Some(jotdown::Event::End($c)) => (),
            _ => bail!("Expected end of container {:?}, got {e:?}", stringify!($c)),
        }
    };
}

/// A block-level element of content, such as `Paragraph` of text or `Footnote`.
#[derive(Serialize, Clone, Debug)]
#[serde(tag = "t", rename_all = "lowercase")]
pub enum Element {
    /// Paragraph, consisting of some text
    Paragraph { text: Vec<InlineElement> },
    /// Code block, with optional language.
    Code {
        lang: Option<String>,
        content: String,
    },
    /// Footnote contents (not to be confused with `FootnoteRef` inline)
    Footnote { tag: String, body: Vec<Element> },
}
impl Element {
    /// Parse several `Element`s from an iterator of jotdown events.
    fn parse_many(events: &mut Peekable<jotdown::Parser>) -> Result<Vec<Self>> {
        type E<'s> = jotdown::Event<'s>;
        type C<'s> = jotdown::Container<'s>;

        // Keep parsing while containers are starting
        let mut elements = vec![];
        loop {
            // Get the next event if we haven't reached the end of file or an enclosing container.
            let Some(e) = events.next_if(|e| !matches!(e, E::End(_))) else {
                break;
            };

            // Parse based on the event we got
            let elem = match e {
                E::Start(C::Paragraph, _) => {
                    let text = InlineElement::parse_many(events)?;
                    assert_container_end!(events, C::Paragraph);
                    Self::Paragraph { text }
                }
                E::Start(C::CodeBlock { language }, _) => {
                    // Get lang
                    let lang = if language.is_empty() {
                        None
                    } else {
                        Some(language.to_string())
                    };

                    // Get contents (must be just one string, will fail on next element otherwise)
                    let Some(content) = InlineElement::parse_text(events) else {
                        bail!("Invalid code block contents")
                    };

                    assert_container_end!(events, C::CodeBlock { .. });
                    Self::Code { lang, content }
                }
                E::Start(C::Footnote { label }, _) => {
                    let body = Element::parse_many(events)?;
                    assert_container_end!(events, C::Footnote { .. });
                    Self::Footnote {
                        tag: label.to_string(),
                        body,
                    }
                }
                E::Blankline => continue,
                E::End(_) => unreachable!(),
                _ => bail!("Got invalid/unsupported event while parsing blocks: {e:?}"),
            };
            elements.push(elem);
        }

        Ok(elements)
    }
}

/// An element appearing inline, as part of text.
#[derive(Serialize, Clone, Debug)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum InlineElement {
    /// Plain text
    Text { content: String },
    /// Emphasized text
    Emph { text: Vec<InlineElement> },
    /// Strongly emphasized text
    Strong { text: Vec<InlineElement> },
    /// A link to some URL (links to tags unsupported)
    Link {
        href: String,
        /// The text to link (just a `Text` for autolinks)
        text: Vec<InlineElement>,
    },
    /// Inline code snippet / verbatim
    InlineCode { content: String },
    /// Footnote reference (must be one-to-one)
    FootnoteRef { number: i32, tag: String },
    /// Embedded image
    Image { src: String, alt: String },
}
impl InlineElement {
    /// Parse several `InlineElement`s from an iterator of jotdown events.
    fn parse_many(events: &mut Peekable<jotdown::Parser>) -> Result<Vec<Self>> {
        type E<'s> = jotdown::Event<'s>;
        type C<'s> = jotdown::Container<'s>;
        use jotdown::{LinkType, SpanLinkType};

        // Keep parsing while containers are starting
        let mut elements = vec![];
        loop {
            // First, get any inline text any add it separately, so we don't need to handle it
            match Self::parse_text(events) {
                Some(content) => elements.push(Self::Text { content }),
                None => (),
            }

            // Get the next event (guarenteed non-text) if we haven't reached the end of file or an enclosing container.
            let Some(e) = events.next_if(|e| !matches!(e, E::End(_))) else {
                break;
            };

            // Parse based on the event we got (know it's not text or container end based on previous checks)
            let elem = match e {
                E::Start(C::Link(url, LinkType::Span(SpanLinkType::Inline)), _) => {
                    let text = InlineElement::parse_many(events)?;
                    assert_container_end!(events, C::Link(_, _));
                    Self::Link {
                        href: url.to_string(),
                        text,
                    }
                }
                E::Start(C::Emphasis, _) => {
                    let text = InlineElement::parse_many(events)?;
                    assert_container_end!(events, C::Emphasis);
                    Self::Emph { text }
                }
                E::Start(C::Strong, _) => {
                    let text = InlineElement::parse_many(events)?;
                    assert_container_end!(events, C::Strong);
                    Self::Strong { text }
                }
                E::Start(C::Verbatim, _) => {
                    // Get contents (must be just one string, will fail on next element otherwise)
                    let Some(content) = InlineElement::parse_text(events) else {
                        bail!("Invalid code block contents")
                    };
                    assert_container_end!(events, C::Verbatim);
                    Self::InlineCode { content }
                }
                E::Start(C::Image(src, SpanLinkType::Inline), _) => {
                    // Get alt text, erroring on end assert if it's not just plain text.
                    let Some(alt) = InlineElement::parse_text(events) else {
                        bail!("No alt text found for image")
                    };
                    assert_container_end!(events, C::Image(_, _));
                    Self::Image {
                        src: src.to_string(),
                        alt,
                    }
                }
                E::FootnoteReference(tag) => Self::FootnoteRef {
                    number: 0,
                    tag: tag.to_string(),
                },
                _ => bail!("Got invalid/unsupported event while parsing inline event: {e:?}"),
            };
            elements.push(elem);
        }

        Ok(elements)
    }
    /// Parses a string from non-container events, combining various special characters with adjacent text.
    ///
    /// Returns `None` if no text is present (a container started or the file/container ended).
    fn parse_text(events: &mut Peekable<jotdown::Parser>) -> Option<String> {
        type E<'s> = jotdown::Event<'s>;
        // Keep parsing and building string until we see non-text.
        let mut result = String::new();
        loop {
            let Some(e) = events.peek() else {
                break;
            };
            // Extract string, or break
            let text = match e {
                E::Str(text) => &text,
                E::EnDash => "–",
                E::EmDash => "—",
                E::LeftDoubleQuote => "“",
                E::RightDoubleQuote => "”",
                E::LeftSingleQuote => "'",
                E::RightSingleQuote => "'",
                _ => break,
            };
            result.push_str(text);
            // Move to the next event
            events.next();
        }
        if result.is_empty() {
            None
        } else {
            Some(result)
        }
    }
}

/// Numbers footnote references and extracts footnotes, warning if some are unmatched.
///
/// Returns a list of the extracted footnotes, numbered starting at 1.
///
/// _NOTE: enforces one-to-one mapping of references to footnotes._
fn extract_footnotes(content: &mut Vec<Element>) -> Result<Vec<(String, Vec<Element>)>> {
    // Keep list of tags we've seen referenced (in order).
    let mut seen_referenced = vec![];

    /// Processes an inline-element, updating the map as references are numbered
    fn number_references(element: &mut InlineElement, seen_referenced: &mut Vec<String>) {
        match element {
            InlineElement::FootnoteRef { number, tag } => {
                *number = seen_referenced.len() as i32 + 1;
                seen_referenced.push(tag.to_string());
            }
            InlineElement::Emph { text } => text
                .iter_mut()
                .for_each(|e| number_references(e, seen_referenced)),
            InlineElement::Strong { text } => text
                .iter_mut()
                .for_each(|e| number_references(e, seen_referenced)),
            InlineElement::Link { text, .. } => text
                .iter_mut()
                .for_each(|e| number_references(e, seen_referenced)),
            InlineElement::Text { .. } => {}
            InlineElement::InlineCode { .. } => {}
            InlineElement::Image { .. } => {}
        }
    }

    // Loop through all elements in the content, extracting footnotes and numbering references.
    // NOTE: because footnotes can contain arbitrary elements, including nested footnotes,
    //       this is incomplete. Modify with a recursive helper if we need nested `Element`s.
    let mut extracted_footnotes = HashMap::new();
    let mut i = 0;
    while i < content.len() {
        let removed_footnote = match &mut content[i] {
            Element::Footnote { tag, body } => Some((std::mem::take(tag), std::mem::take(body))),
            Element::Paragraph { text } => {
                text.iter_mut()
                    .for_each(|e| number_references(e, &mut seen_referenced));
                None
            }
            Element::Code { .. } => None,
        };
        match removed_footnote {
            Some((tag, body)) => {
                // This element is a footnote, remove it and add to the map
                if extracted_footnotes.insert(tag.clone(), body).is_some() {
                    bail!("Duplicate footnote for tag {tag}")
                }
                content.remove(i);
            }
            None => {
                // Not a footnote, move on
                i += 1;
            }
        }
    }

    // Create a sorted list of extracted footnotes, removing as we go to check one-to-one mapping
    let mut footnotes = vec![];
    for tag in seen_referenced {
        match extracted_footnotes.remove(&tag) {
            Some(body) => {
                footnotes.push((tag, body));
            }
            None => bail!("No footnote for reference {tag}"),
        }
    }
    // Check we don't have any unreferenced footnotes
    if let Some((leftover_tag, _)) = extracted_footnotes.iter().next() {
        bail!("Found unreferenced footnote {leftover_tag}");
    }
    Ok(footnotes)
}

// Display implementation for converting posts to strings
impl std::fmt::Display for BlogPost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Self {
            title,
            url,
            date,
            content,
            visibility: _,
            tags: _,
        } = self;
        writeln!(f, "=== {} ===", title)?;
        writeln!(f, "https://{}/projects/{}", crate::CONFIG.domain, url)?;
        writeln!(f, "{}", date.date())?;
        writeln!(f, "\n{}", content)
    }
}
impl std::fmt::Display for Content {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Self { content, footnotes } = self;
        writeln!(f, "{}", content.to_string())?;
        if !footnotes.is_empty() {
            writeln!(f, "Footnotes:")?;
            for (i, note) in footnotes.iter().enumerate() {
                write!(f, "[^{}]: {}", i + 1, note.1.to_string())?;
            }
        }
        Ok(())
    }
}
impl std::fmt::Display for Element {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Element::Paragraph { text } => writeln!(f, "{}", text.to_string()),
            Element::Code { content, .. } => writeln!(f, "```\n{content}\n```"),
            Element::Footnote { .. } => writeln!(f, "BUG: footnote"),
        }
    }
}
impl std::fmt::Display for InlineElement {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InlineElement::Text { content } => write!(f, "{content}"),
            InlineElement::Emph { text } => write!(f, "_{}_", text.to_string()),
            InlineElement::Strong { text } => write!(f, "*{}*", text.to_string()),
            InlineElement::Link { href, text } => write!(f, "[{}]({href})", text.to_string()),
            InlineElement::InlineCode { content } => write!(f, "`{content}`"),
            InlineElement::FootnoteRef { number, .. } => write!(f, "[{number}]"),
            InlineElement::Image { src, alt } => write!(f, "<Image: {alt} ({src})>"),
        }
    }
}
trait VecFormat {
    fn to_string(&self) -> String;
}
impl VecFormat for Vec<Element> {
    fn to_string(&self) -> String {
        self.iter()
            .map(|e| e.to_string() + "\n")
            .collect::<String>()
    }
}
impl VecFormat for Vec<InlineElement> {
    fn to_string(&self) -> String {
        self.iter().map(|e| e.to_string()).collect::<String>()
    }
}
