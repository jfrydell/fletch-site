use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};

/// One blog post and all of its content and metadata.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct BlogPost {
    pub title: String,
    pub url: String,
    pub date: NaiveDateTime,

    pub content: String,
}
