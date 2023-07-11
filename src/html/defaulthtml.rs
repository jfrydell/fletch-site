use std::{collections::HashMap, sync::Arc};

use anyhow::Result;
use tera::Tera;

/// Stores the rendered basic HTML content, for serving previews or writing to files.
pub struct Content {
    /// `index.html` contents
    pub index: String,
    /// Contents of `projects/` indexed by name
    pub projects: HashMap<String, String>,
    /// CSS generated by railwind for all rendered content
    pub css: String,
    /// Templating engine
    pub tera: Tera,
}

impl Content {
    /// Renders the default HTML pages from the general content.
    pub async fn new(content: &crate::Content) -> Result<Self> {
        // The template engine is the only thing that must be loaded for html-specific content, so load that first.
        let tera = Tera::new("defaulthtml-templates/**/*.tera").expect("Failed to load templates");

        // To render the content, we just create an empty struct and call the refresh function with the content.
        let mut result = Self {
            index: String::new(),
            projects: HashMap::new(),
            css: String::new(),
            tera,
        };
        result.refresh(content).await?;
        Ok(result)
    }

    /// Rerender the basic HTML from the given content.
    pub async fn refresh(&mut self, content: &crate::Content) -> Result<()> {
        // Make index page
        let context = tera::Context::from_serialize(content)?;
        self.index = self.tera.render("index.tera", &context)?;

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

        // Make CSS
        self.make_css();

        Ok(())
    }

    /// Makes the css string from the current rendered content.
    pub fn make_css(&mut self) {
        use railwind::*;
        // Concatenate all html files together for railwind to parse.
        let mut html = self.index.clone();
        for (_, project) in self.projects.iter() {
            html.push_str(project);
        }
        // Parse html string (just an regex match internally, so concatenated html is fine)
        self.css = parse_to_string(
            Source::String(html, CollectionOptions::Html),
            true,
            &mut vec![],
        );

        // Hijack dark mode to use the "class" strategy
        while let Some(i) = self.css.find("@media (prefers-color-scheme: dark) {") {
            // Replace media selector with class selector
            let line_end = self.css[i..]
                .find("\n")
                .expect("No newline after dark mode selector");
            self.css
                .replace_range(i..i + line_end, "@media screen { .dark");
        }
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
                    content.content.read().await.default.css.clone(),
                )
            }),
        )
    }
}