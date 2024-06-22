use chrono::NaiveDateTime;
use color_eyre::Result;
use tera::Tera;

/// Generates and stores the Atom feed.
#[derive(Default)]
pub struct Feed {
    tera: Tera,
    atom: String,
}

impl Feed {
    /// Renders the feed XML from the general content.
    pub fn new(content: &crate::Content) -> Result<Self> {
        // The template engine is the only thing that must be loaded for html-specific content, so load that first.
        let mut tera = Tera::default();
        tera.add_template_file("html-content/feed.tera", Some("atom"))?;
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
        // Find updated date
        let mut updated =
            NaiveDateTime::parse_from_str("2024-01-19T18:00:00", "%Y-%m-%dT%H:%M:%S").unwrap();
        for post in content.blog_posts.iter() {
            if post.date > updated {
                updated = post.date;
            }
        }

        // Make atom feed
        let mut context = tera::Context::from_serialize(content)?;
        context.insert("updated", &updated);
        self.atom = self.tera.render("atom", &context)?;
        Ok(())
    }

    pub fn atom(&self) -> String {
        self.atom.clone()
    }
}
