use std::fmt::Display;

use serde::{Deserialize, Serialize};

/// One project and all of its content and metadata.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Project {
    pub name: String,
    pub url: String,
    pub description: String,
    pub date: String,
    pub content: Content,
    pub thumbnail: String,
    pub skills: Skills,
    /// The priority of this project, used for sorting. Non-positive priority projects are hidden by default.
    pub priority: i32,
}
impl Display for Project {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let &Self {
            ref name,
            ref url,
            ref description,
            ref date,
            ref content,
            thumbnail: ref _thumbnail,
            ref skills,
            priority: ref _priority,
        } = self;
        // Header
        writeln!(f, "=== {} ===", name)?;
        writeln!(f, "https://{}/projects/{}", crate::CONFIG.domain, url)?;
        writeln!(f, "{}", description)?;
        writeln!(f, "{}", date)?;
        writeln!(f, "Skills:")?;
        for skill in skills.skills.iter() {
            writeln!(f, "- {}", skill)?;
        }
        writeln!(
            f,
            "{}\n",
            String::from_iter(std::iter::repeat('=').take(name.len() + 8))
        )?;
        // Content
        write!(f, "{}", content)?;

        Ok(())
    }
}

/// The content of a project, including several `Section`s.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Content {
    #[serde(rename = "$value", default)]
    pub sections: Vec<Section>,
}
impl Display for Content {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for section in self.sections.iter() {
            writeln!(f, "{}\n", section)?;
        }
        Ok(())
    }
}

/// A section of a project, such as a general-purpose `Section::Section` of content or a special `Section::Criteria` section listing design criteria.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Section {
    /// A generic section, consisting of an optional title and some content.
    Section {
        title: Option<String>,
        #[serde(rename = "$value")]
        content: Vec<Element>,
    },
    /// A section listing design criteria, consisting of an optional title and a list of criteria, each containing a title and description.
    Criteria {
        title: Option<String>,
        #[serde(rename = "item")]
        items: Vec<TitleDesc>,
    },
}
impl Display for Section {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Section::Section { title, content } => {
                writeln!(
                    f,
                    "# {}",
                    title.as_ref().map(|s| &s[..]).unwrap_or("Section")
                )?;
                let mut newline = false;
                for element in content.iter() {
                    if newline {
                        writeln!(f)?;
                    }
                    write!(f, "{}", element)?;
                    newline = true;
                }
            }
            Section::Criteria { title, items } => {
                write!(
                    f,
                    "# {}",
                    title.as_ref().map(|s| &s[..]).unwrap_or("Design Criteria")
                )?;
                for item in items.iter() {
                    writeln!(f, "\n## {}", item.title)?;
                    writeln!(f, "{}", item.description)?;
                }
            }
        }
        Ok(())
    }
}

/// A paired title and description.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct TitleDesc {
    pub title: String,
    pub description: Text,
}

/// A list of skills, each a string.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Skills {
    #[serde(rename = "skill", default)]
    pub skills: Vec<String>,
}

/// A single element of content, such as a `Group` of other elements or a `Paragraph` of text.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum Element {
    /// A generic group of sequential elements.
    #[serde(rename = "g")]
    Group {
        #[serde(rename = "$value")]
        content: Vec<Element>,
    },
    /// A gallery of many equal-weight elements.
    #[serde(rename = "gallery")]
    Gallery {
        #[serde(rename = "$value")]
        content: Vec<Element>,
    },
    #[serde(rename = "p")]
    Paragraph(Text),
    #[serde(rename = "img")]
    Image {
        #[serde(rename = "@src")]
        src: String,
        #[serde(rename = "@alt")]
        alt: String,
        caption: Option<Text>,
    },
}
impl Display for Element {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Element::Group { content } => {
                let mut newline = false;
                for element in content.iter() {
                    if newline {
                        writeln!(f)?;
                    }
                    write!(f, "{}", element)?;
                    newline = true;
                }
            }
            Element::Gallery { content } => {
                for element in content.iter() {
                    write!(f, "{}", element)?;
                }
            }
            Element::Paragraph(text) => writeln!(f, "{}", text)?,
            Element::Image { src, alt, caption } => {
                writeln!(f, "Image: {alt} ({src})")?;
                if let Some(caption) = caption {
                    writeln!(f, "Caption: {}", caption)?;
                }
            }
        }
        Ok(())
    }
}

/// Some text, consisting of several `TextElement`s.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Text {
    #[serde(rename = "$value", default)]
    pub text: Vec<TextElement>,
}
impl Display for Text {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for element in self.text.iter() {
            write!(f, "{}", element)?;
        }
        Ok(())
    }
}

/// A single element of text, a piece of text or hypertext (such as a link).
#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum TextElement {
    #[serde(rename = "a")]
    Link {
        #[serde(rename = "@href")]
        href: String,
        #[serde(rename = "@lead", default = "space")]
        leading_space: String,
        #[serde(rename = "@trail", default = "space")]
        trailing_space: String,
        #[serde(rename = "$value")]
        text: Vec<TextElement>,
    },
    #[serde(rename = "$text")]
    Text(String),
}
fn space() -> String {
    " ".to_string()
}
impl Display for TextElement {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TextElement::Link {
                href,
                text,
                leading_space,
                trailing_space,
            } => {
                write!(f, "{leading_space}[")?;
                for element in text.iter() {
                    write!(f, "{}", element)?;
                }
                write!(f, "]({href}){trailing_space}")?;
            }
            TextElement::Text(text) => write!(f, "{}", text)?,
        }
        Ok(())
    }
}
