use std::{collections::HashMap, sync::Arc};

use color_eyre::Result;
use tera::Tera;

/// Stores the rendered basic HTML content, for serving previews or writing to files.
#[derive(Default)]
pub struct Content {
    /// `index.html` contents
    pub index: String,
    /// `themes.html` contents
    pub themes: String,
    /// Contents of `projects/` indexed by name
    pub projects: HashMap<String, String>,
    /// Contents of `blog/` indexed by name
    pub blog: HashMap<String, String>,
    /// CSS loaded from a file
    pub css: String,
    /// Templating engine
    pub tera: Tera,
}

impl Content {
    /// Renders the simple HTML pages from the general content.
    pub fn new(content: &crate::Content) -> Result<Self> {
        // The template engine is the only thing that must be loaded for html-specific content, so load that first.
        let mut tera = Tera::new("html-content/simple/**/*.tera")?;
        tera.autoescape_on(vec![".tera"]);

        // To render the content, we just create an empty struct and call the refresh function with the content.
        let mut result = Self {
            tera,
            ..Default::default()
        };
        result.refresh(content)?;
        Ok(result)
    }

    /// Rerender the simple HTML from the given content.
    pub fn refresh(&mut self, content: &crate::Content) -> Result<()> {
        // Make index page
        let context = tera::Context::from_serialize(content)?;
        self.index = self.tera.render("index.tera", &context)?;

        // Make themes page
        self.themes = self.tera.render(
            "themes.tera",
            &tera::Context::from_serialize(&content.themes_info)?,
        )?;

        // Make project pages
        self.projects = HashMap::new();
        for project in content.projects.iter() {
            let mut context = tera::Context::new();
            context.insert("project", &project);
            self.projects.insert(
                project.url.clone(),
                self.tera.render("project.tera", &context)?,
            );
        }

        // Make blog pages
        self.blog = HashMap::new();
        for blog_post in content.blog_posts.iter() {
            let mut context = tera::Context::new();
            context.insert("post", &blog_post);
            self.blog.insert(
                blog_post.url.clone(),
                self.tera.render("blogpost.tera", &context)?,
            );
        }

        // Load CSS
        self.css = std::fs::read_to_string("html-content/simple/css.css")?;

        Ok(())
    }

    /// Get a page, optionally with "pure" mode (no CSS).
    pub fn get_page(&self, page: &super::Page, pure: bool) -> Option<String> {
        use super::Page::*;
        match page {
            Index => Some(self.index.clone()),
            Themes => Some(self.themes.clone()),
            Project(name) => self.projects.get(name).cloned(),
            BlogPost(name) => self.blog.get(name).cloned(),
            _ => None,
        }
        .map(|page| {
            if pure {
                page.replace(
                    r#"<link rel="stylesheet" href="/simplehtml/css.css" type="text/css">"#,
                    "",
                )
            } else {
                page
            }
        })
    }

    /// Serve the css.
    pub fn router() -> axum::Router<Arc<super::HtmlServer>> {
        use axum::{extract::State, routing::get, Router};
        use hyper::header::CONTENT_TYPE;
        Router::new().route(
            "/css.css",
            get(|State(content): State<Arc<super::HtmlServer>>| async move {
                (
                    [(CONTENT_TYPE, "text/css")],
                    content.content.read().await.simple.css.clone(),
                )
            }),
        )
    }
}
