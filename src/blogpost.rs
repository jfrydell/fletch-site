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
}

/// Deserialization for blog post content from a string.
fn deserialize_content<'de, D>(de: D) -> Result<Content, D::Error>
where
    D: serde::Deserializer<'de>,
{
    // Parse the XML body into a String as-is, then parse as jdot event stream.
    let raw = String::deserialize(de)?; //.content;
    let mut events = jotdown::Parser::new(&raw).peekable();
    let elements = Element::parse_many(&mut events)
        .map_err(|e| serde::de::Error::custom(format!("error deserializing post content: {e}")))?;

    // TODO: number footnotes
    dbg!(&elements);

    Ok(Content { content: elements })
}

// Utility macro to check ending event matches the given container, bailing if not.
macro_rules! assert_container_end {
    ($e:expr, $c:pat) => {
        let e = $e.next();
        match e {
            Some(jotdown::Event::End($c)) => (),
            _ => bail!("Expected container end, got {e:?}"),
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
    Footnote {
        number: i32,
        tag: String,
        text: Vec<InlineElement>,
    },
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
                    let text = InlineElement::parse_many(events)?;
                    assert_container_end!(events, C::Paragraph);
                    // Set number to 0 for now, top level will set refs and footnotes correctly
                    Self::Footnote {
                        number: 0,
                        tag: label.to_string(),
                        text,
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
    /// A link to some URL (links to tags unsupported)
    Link {
        href: String,
        /// The text to link (just a `Text` for autolinks)
        text: Vec<InlineElement>,
    },
    /// Inline code snippet / verbatim
    InlineCode { content: String },
    /// Footnote reference
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
