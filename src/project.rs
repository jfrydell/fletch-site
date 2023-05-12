use liquid::{model::Value, Object};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct Project {
    pub name: String,
    pub url: String,
    pub description: String,
    pub content: String,
    pub thumbnail: String,
    /// The priority of this project, used for sorting.
    pub priority: i32,
}

impl Project {
    pub fn to_liquid(&self) -> Object {
        let mut obj = Object::new();
        obj.insert("name".into(), Value::scalar(self.name.clone()));
        obj.insert("url".into(), Value::scalar(self.url.clone()));
        obj.insert(
            "description".into(),
            Value::scalar(self.description.clone()),
        );
        obj.insert("thumbnail".into(), Value::scalar(self.thumbnail.clone()));
        obj.insert("content".into(), Value::scalar(self.content.clone()));
        obj
    }
}
