use std::collections::HashMap;

use chrono::NaiveDateTime;
use color_eyre::{eyre::eyre, Result};
use serde::{Deserialize, Serialize};

/// One blog post and all of its content and metadata.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct BlogPost {
    pub title: String,
    pub url: String,
    pub date: NaiveDateTime,
    pub visibility: i32,

    #[serde(deserialize_with = "deserialize_content")]
    pub content: Content,
}

/// The content of a blog post, including the `Element`s that make it up as well as footnotes.
#[derive(Serialize, Debug, Clone)]
pub struct Content {
    /// The body of the content, consisting of several `Element`s.
    content: Vec<Element>,
    /// The footnotes of the content, indexed by `ref`.
    footnotes: HashMap<String, Vec<Element>>,
}

/// The XML "as-is" content of a project, with no processing of shorthands or footnotes.
#[derive(Deserialize, Clone, Debug)]
pub struct ContentBody {
    #[serde(rename = "$value", default)]
    sections: Vec<Element>,
}

/// Deserialization for blog post content from a string.
fn deserialize_content<'de, D>(de: D) -> Result<Content, D::Error>
where
    D: serde::Deserializer<'de>,
{
    // Parse the XML into a struct as-is.
    let mut sections = ContentBody::deserialize(de)?.sections;

    // Pull out the footnotes and store them in a map.
    let mut footnotes = HashMap::new();
    sections.retain(|element| match element {
        Element::Footnote { reference, content } => {
            footnotes.insert(reference.clone(), content.clone());
            false
        }
        _ => true,
    });

    // Process any shorthand in the content.
    expand_shorthand(&mut sections).map_err(|e| {
        use serde::de::Error;
        D::Error::custom(format!(
            "Error expanding shorthand in blog post content: {e}"
        ))
    })?;

    Ok(Content {
        content: sections,
        footnotes: HashMap::new(),
    })
}

/// Expands out any shorthand in the given `Element`s.
fn expand_shorthand(elements: &mut Vec<Element>) -> Result<()> {
    let mut element_i = 0;
    while element_i < elements.len() {
        if matches!(elements[element_i], Element::Text(..)) {
            // Search for shorthand in the text
            let Element::Text(text) = &mut elements[element_i] else {
                unreachable!()
            };
            // Link shorthand (TODO: use regex so [ is allowed in text)
            if let Some(i) = text.find('[') {
                // Grab link and the rest of the text for processing
                let link_and_rest = &text[i..];

                // Construct link element
                let link_end = link_and_rest.find(']').ok_or(eyre!("No ] closing link"))?;
                let link_content: ContentBody = quick_xml::de::from_str(&format!(
                    "<?xml version=\"1.0\"?><a>{}</a>",
                    &link_and_rest[1..link_end]
                ))?;
                let href_and_rest = &link_and_rest[link_end + 1..];
                let href_end = href_and_rest.find(')').ok_or(eyre!("No ) closing link"))?;
                let href = &href_and_rest[1..href_end].to_string();
                let link = Element::Link {
                    href: href.clone(),
                    leading_space: "".to_string(),
                    trailing_space: "".to_string(),
                    text: link_content.sections,
                };

                // Construct subsequent text
                let rest = Element::Text(href_and_rest[href_end + 1..].to_string());

                // Add new elements after current and truncate current to prepare for next iteration
                text.truncate(i);
                elements.insert(element_i + 1, link);
                elements.insert(element_i + 2, rest);
            }
        }

        // Recursive descent into any elements containing text
        match &mut elements[element_i] {
            Element::Link { ref mut text, .. } => expand_shorthand(text)?,
            Element::Footnote {
                ref mut content, ..
            } => expand_shorthand(content)?,
            Element::Text(..) => {}
            Element::Code { .. } => {}
            Element::InlineCode { .. } => {}
            Element::FootnoteRef { .. } => {}
        };
        element_i += 1;
    }
    Ok(())
}

/// A single element of content, such as a `Group` of other elements or a `Paragraph` of text.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum Element {
    /// A link, with leading and trailing spaces (bad hack).
    #[serde(rename = "a")]
    Link {
        #[serde(rename = "@href")]
        href: String,
        #[serde(rename = "@lead", default = "space")]
        leading_space: String,
        #[serde(rename = "@trail", default = "space")]
        trailing_space: String,
        #[serde(rename = "$value")]
        text: Vec<Element>,
    },
    /// Inline code snippet, with optional language.
    #[serde(rename = "c")]
    InlineCode {
        #[serde(rename = "@lang")]
        lang: Option<String>,
        #[serde(rename = "@lead", default = "space")]
        leading_space: String,
        #[serde(rename = "@trail", default = "space")]
        trailing_space: String,
        #[serde(rename = "$text")]
        text: String,
    },
    /// Code block, with optional language.
    #[serde(rename = "cb")]
    Code {
        #[serde(rename = "@lang")]
        lang: Option<String>,
        #[serde(rename = "$text")]
        text: String,
    },
    /// Footnote reference.
    #[serde(rename = "fnref")]
    FootnoteRef {
        #[serde(rename = "@ref")]
        reference: String,
    },
    /// Footnote definition.
    #[serde(rename = "footnote")]
    Footnote {
        #[serde(rename = "@ref")]
        reference: String,
        #[serde(rename = "$value")]
        content: Vec<Element>,
    },
    #[serde(rename = "$text")]
    Text(String),
}
fn space() -> String {
    " ".to_string()
}
