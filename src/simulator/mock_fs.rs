use crate::model::{LocalFileId, LocalNode};
use md5::{Digest, Md5};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone)]
pub struct MockFile {
    pub content: Vec<u8>,
    pub md5sum: String,
}

#[derive(Debug, Clone, Default)]
pub struct MockFs {
    pub files: HashMap<LocalFileId, MockFile>,
    pub dirs: HashSet<LocalFileId>,
    pub nodes: HashMap<LocalFileId, LocalNode>,
}

impl MockFs {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn create_file(&mut self, id: LocalFileId, node: LocalNode, content: Vec<u8>) {
        let mut hasher = Md5::new();
        hasher.update(&content);
        let md5sum = format!("{:x}", hasher.finalize());
        self.files.insert(id.clone(), MockFile { content, md5sum });
        self.nodes.insert(id, node);
    }

    pub fn create_dir(&mut self, id: LocalFileId, node: LocalNode) {
        self.dirs.insert(id.clone());
        self.nodes.insert(id, node);
    }

    #[must_use]
    pub fn read_file(&self, id: &LocalFileId) -> Option<&Vec<u8>> {
        self.files.get(id).map(|f| &f.content)
    }

    pub fn delete(&mut self, id: &LocalFileId) {
        self.files.remove(id);
        self.dirs.remove(id);
        self.nodes.remove(id);
    }

    #[must_use]
    pub fn exists(&self, id: &LocalFileId) -> bool {
        self.files.contains_key(id) || self.dirs.contains(id)
    }

    #[must_use]
    pub fn get_node(&self, id: &LocalFileId) -> Option<&LocalNode> {
        self.nodes.get(id)
    }

    pub fn move_node(
        &mut self,
        id: &LocalFileId,
        new_parent: Option<LocalFileId>,
        new_name: String,
    ) {
        if let Some(node) = self.nodes.get_mut(id) {
            node.parent_id = new_parent;
            node.name = new_name;
        }
    }

    #[must_use]
    pub fn list_all(&self) -> Vec<&LocalNode> {
        self.nodes.values().collect()
    }
}
