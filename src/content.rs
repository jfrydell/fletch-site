use crate::eyre;
use crate::{blogpost, project};
use color_eyre::Result;
use serde::Serialize;
use tracing::info;

#[derive(Serialize)]
pub struct Content {
    pub projects: Vec<project::Project>,
    pub blog_posts: Vec<blogpost::BlogPost>,
    pub index_info: serde_json::Value,
    pub themes_info: serde_json::Value,
}
impl Content {
    /// Loads all content from the `content/` directory.
    pub async fn load() -> Result<Content> {
        // Load index and themes info
        let index_info =
            serde_json::from_str(&tokio::fs::read_to_string("content/index.json").await?)?;
        let themes_info =
            serde_json::from_str(&tokio::fs::read_to_string("content/themes.json").await?)?;

        Ok(Content {
            projects: Self::load_projects().await?,
            blog_posts: Self::load_blog_posts().await?,
            index_info,
            themes_info,
        })
    }

    /// Loads all projects from the `content/projects/` directory.
    async fn load_projects() -> Result<Vec<project::Project>> {
        // Get list of all projects from `content/projects`
        let mut projects = Vec::new();
        let mut entries = tokio::fs::read_dir("content/projects").await.unwrap();
        while let Some(entry) = entries.next_entry().await.unwrap() {
            let path = entry.path();
            if path.is_file() {
                // Load project
                let project: project::Project = quick_xml::de::from_reader(
                    std::io::BufReader::new(std::fs::File::open(path)?),
                )?;
                info!("Loaded project: {}", project.name);
                projects.push(project);
            }
        }
        projects.sort_by_key(|p| -p.priority);

        // If we disabled hidden projects, remove any with priority <= 0
        if !crate::CONFIG.show_hidden {
            projects.retain(|p| p.priority > 0);
        }

        // Verify that project urls and priorities are unique
        Self::verify_unique(
            &projects.iter().map(|p| &p.url).collect::<Vec<_>>(),
            "project url",
        )?;
        Self::verify_unique(
            &projects
                .iter()
                .filter(|p| p.priority > 0)
                .map(|p| &p.priority)
                .collect::<Vec<_>>(),
            "project priority",
        )?;

        Ok(projects)
    }

    /// Loads all blog posts from the `content/blog/` directory.
    async fn load_blog_posts() -> Result<Vec<blogpost::BlogPost>> {
        // Get list of all blog posts from `content/blog`
        let mut blog_posts = Vec::new();
        let mut entries = tokio::fs::read_dir("content/blog").await.unwrap();
        while let Some(entry) = entries.next_entry().await.unwrap() {
            let path = entry.path();
            if path.is_file() {
                // Load post
                let blog_post: blogpost::BlogPost = quick_xml::de::from_reader(
                    std::io::BufReader::new(std::fs::File::open(path)?),
                )?;
                blog_posts.push(blog_post);
            }
        }
        blog_posts.sort_by_key(|p| p.date);
        info!("Loaded {} blog posts", blog_posts.len());

        // Verify that blog post urls and dates are unique
        Self::verify_unique(
            &blog_posts.iter().map(|p| &p.url).collect::<Vec<_>>(),
            "blog post url",
        )?;
        Self::verify_unique(
            &blog_posts.iter().map(|p| &p.date).collect::<Vec<_>>(),
            "blog post date",
        )?;

        Ok(blog_posts)
    }

    /// Helper to that a `Vec` has no duplicates, for checking uniqueness of identifiers.
    fn verify_unique<T: std::cmp::Eq + std::hash::Hash + std::fmt::Debug>(
        vec: &Vec<T>,
        name: &str,
    ) -> Result<()> {
        let mut set = std::collections::HashSet::new();
        for item in vec {
            if !set.insert(item) {
                return Err(eyre!("Duplicate {} found: {:?}", name, item));
            }
        }
        Ok(())
    }
}
