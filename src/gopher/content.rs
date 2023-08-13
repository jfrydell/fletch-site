//! Defines all the Gopher content through the `GopherContent` trait, which allows any type to be converted to a Gopher menu.
//!
//! Note that all types of content are converted to menus, even when they are TXT files in the SSH version. This is to support embedded
//! images and links. For projects, the idea is to supply a link to the TXT version, which is rendered using the `Display` trait.

use color_eyre::eyre::eyre;
use color_eyre::Result;
use gophermap::{GopherMenu, ItemType};

/// A trait enabling a type to be written to a `gophermap` menu.
pub trait GopherContent {
    /// Write the content to the given Gopher menu.
    fn gopher<'a, W>(&self, menu: &GopherMenu<&'a W>) -> Result<()>
    where
        &'a W: std::io::Write;
}

macro_rules! access_json {
    ($content:expr, $path:expr) => {
        $content
            .get($path)
            .and_then(|v| v.as_str())
            .ok_or_else(|| eyre!("No {} found in {}", stringify!($path), stringify!($content)))?
    };
}

impl GopherContent for crate::Content {
    fn gopher<'a, W>(&self, menu: &GopherMenu<&'a W>) -> Result<()>
    where
        &'a W: std::io::Write,
    {
        let intro_info = format!("
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

RRRRRRRRRRRRR       YYYY      YYYY     DDDDDDDDDDDD        EEEEEEEEEEEEEE     LLLL               LLLL           
RRRRRRRRRRRRRRR      YYYY    YYYY      DDDDDDDDDDDDDD      EEEEEEEEEEEEEE     LLLL               LLLL           
RRRR       RRRR       YYYY  YYYY       DDDD       DDDD     EEEE               LLLL               LLLL           
RRRR       RRRR        YYYYYYYY        DDDD       DDDD     EEEE               LLLL               LLLL           
RRRRRRRRRRRRRRR         YYYYYY         DDDD       DDDD     EEEEEEEEEEEEEE     LLLL               LLLL           
RRRRRRRRRRRRR            YYYY          DDDD       DDDD     EEEEEEEEEEEEEE     LLLL               LLLL           
RRRRRRRRRR               YYYY          DDDD       DDDD     EEEE               LLLL               LLLL           
RRRR   RRRRR             YYYY          DDDD       DDDD     EEEE               LLLL               LLLL           
RRRR     RRRRR           YYYY          DDDDDDDDDDDDDD      EEEEEEEEEEEEEE     LLLLLLLLLLLLLL     LLLLLLLLLLLLLLL
RRRR      RRRRR          YYYY          DDDDDDDDDDDD        EEEEEEEEEEEEEE     LLLLLLLLLLLLLL     LLLLLLLLLLLLLLL

Hello there! My name is Fletch Rydell (see above as long as your terminal width >= 112), and I'd like to welcome you to my site.
Not too many people use Gopher these days, so I'm glad you're here.

This site is a mirror of my HTTP-based and SSH-based sites, and should contain all the same content, just in the superior Gopher format.
I hope you enjoy it!

# Fletch Rydell
{}

## About me
{}

## Projects
{}", access_json!(self.index_info, "subtitle"), access_json!(self.index_info, "about_me"), access_json!(self.index_info, "projects_caption"));

        // Insert the intro info into the menu.
        for line in intro_info.lines() {
            menu.info(line)?;
        }

        // List projects in the menu
        for project in self.projects.iter() {
            menu.write_entry(
                ItemType::Directory,
                &format!("{} - {}", project.name, project.description),
                &format!("/projects/{}", project.url),
                &crate::CONFIG.domain,
                crate::CONFIG.gopher_port,
            )?;
        }

        Ok(())
    }
}

impl GopherContent for crate::project::Project {
    fn gopher<'a, W>(&self, menu: &GopherMenu<&'a W>) -> Result<()>
    where
        &'a W: std::io::Write,
    {
        let &Self {
            ref name,
            ref url,
            ref description,
            ref date,
            ref content,
            ref thumbnail,
            ref skills,
            priority: ref _priority,
        } = self;
        // Header
        menu.info(&format!("=== {} ===", name))?;
        menu.info(&format!("{}", description))?;
        menu.info(&format!("{}", date))?;
        menu.write_entry(
            ItemType::File,
            "(Plaintext version)",
            &format!("/projects/{}.txt", url),
            &crate::CONFIG.domain,
            crate::CONFIG.gopher_port,
        )?;
        menu.write_entry(
            ItemType::Image,
            "(Thumbnail)",
            &format!("/images/{}", thumbnail),
            &crate::CONFIG.domain,
            crate::CONFIG.gopher_port,
        )?;
        menu.info("Skills:")?;
        for skill in skills.skills.iter() {
            menu.info(&format!("- {}", skill))?;
        }
        menu.info(&String::from_iter(
            std::iter::repeat('=').take(name.len() + 8),
        ))?;

        // Content
        content.gopher(menu)?;

        Ok(())
    }
}

impl GopherContent for crate::project::Content {
    fn gopher<'a, W>(&self, menu: &GopherMenu<&'a W>) -> Result<()>
    where
        &'a W: std::io::Write,
    {
        for section in self.sections.iter() {
            menu.info("")?;
            section.gopher(menu)?;
            menu.info("")?;
        }
        Ok(())
    }
}

impl GopherContent for crate::project::Section {
    fn gopher<'a, W>(&self, menu: &GopherMenu<&'a W>) -> Result<()>
    where
        &'a W: std::io::Write,
    {
        use crate::project::Section;
        match self {
            Section::Section { title, content } => {
                menu.info(&format!(
                    "# {}",
                    title.as_ref().map(|s| &s[..]).unwrap_or("Section")
                ))?;
                let mut newline = false;
                for element in content.iter() {
                    if newline {
                        menu.info("")?;
                    }
                    element.gopher(menu)?;
                    newline = true;
                }
            }
            Section::Criteria { title, items } => {
                menu.info(&format!(
                    "# {}",
                    title.as_ref().map(|s| &s[..]).unwrap_or("Design Criteria")
                ))?;
                let mut newline = false;
                for item in items.iter() {
                    if newline {
                        menu.info("")?;
                    }
                    menu.info(&format!("## {}", item.title))?;
                    item.description.gopher(menu)?;
                    newline = true;
                }
            }
        }
        Ok(())
    }
}

impl GopherContent for crate::project::Element {
    fn gopher<'a, W>(&self, menu: &GopherMenu<&'a W>) -> Result<()>
    where
        &'a W: std::io::Write,
    {
        use crate::project::Element;
        match self {
            Element::Group { content } => {
                let mut newline = false;
                for element in content.iter() {
                    if newline {
                        menu.info("")?;
                    }
                    element.gopher(menu)?;
                    newline = true;
                }
            }
            Element::Gallery { content } => {
                for element in content.iter() {
                    element.gopher(menu)?;
                }
            }
            Element::Paragraph(text) => text.gopher(menu)?,
            Element::Image { src, alt, caption } => {
                menu.write_entry(
                    ItemType::Image,
                    &format!("Image: {}", alt),
                    &format!("/images/{src}"),
                    &crate::CONFIG.domain,
                    crate::CONFIG.gopher_port,
                )?;
                if let Some(caption) = caption {
                    menu.info("Caption:")?;
                    caption.gopher(menu)?;
                }
            }
        }
        Ok(())
    }
}

impl GopherContent for crate::project::Text {
    fn gopher<'a, W>(&self, menu: &GopherMenu<&'a W>) -> Result<()>
    where
        &'a W: std::io::Write,
    {
        for element in self.text.iter() {
            element.gopher(menu)?;
        }
        Ok(())
    }
}

impl GopherContent for crate::project::TextElement {
    fn gopher<'a, W>(&self, menu: &GopherMenu<&'a W>) -> Result<()>
    where
        &'a W: std::io::Write,
    {
        use crate::project::TextElement;
        match self {
            TextElement::Link { href, text, .. } => {
                let raw_text = text.iter().map(|e| e.to_string()).collect::<String>();
                if href.starts_with("https://") {
                    menu.write_entry(
                        ItemType::Other('h'),
                        &format!("{raw_text} (External Link: {href})"),
                        &format!("URL:{}", href),
                        &crate::CONFIG.domain,
                        crate::CONFIG.gopher_port,
                    )?;
                } else {
                    menu.write_entry(
                        ItemType::Directory,
                        &raw_text,
                        href,
                        &crate::CONFIG.domain,
                        crate::CONFIG.gopher_port,
                    )?;
                }
            }
            TextElement::Text(text) => menu.info(&text)?,
        }
        Ok(())
    }
}
