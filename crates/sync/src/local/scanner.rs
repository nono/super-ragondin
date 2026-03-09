use crate::error::Result;
use crate::model::{LocalFileId, LocalNode, NodeType};
use crate::util::compute_md5_from_path;
use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

pub struct Scanner {
    root: PathBuf,
}

impl Scanner {
    #[must_use]
    pub fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
        }
    }

    /// Scan all files and directories under the root.
    ///
    /// # Errors
    /// Returns an error if filesystem operations fail.
    pub fn scan(&self) -> Result<Vec<LocalNode>> {
        tracing::info!(root = %self.root.display(), "🔍 Starting local filesystem scan");
        let mut nodes = Vec::new();
        let mut inode_to_id: HashMap<(u64, u64), LocalFileId> = HashMap::new();

        let root_meta = fs::symlink_metadata(&self.root)?;
        let root_id = LocalFileId::new(root_meta.dev(), root_meta.ino());

        Self::scan_recursive(&self.root, Some(&root_id), &mut nodes, &mut inode_to_id)?;
        tracing::info!(root = %self.root.display(), count = nodes.len(), "🔍 Scan complete");
        Ok(nodes)
    }

    fn scan_recursive(
        path: &Path,
        parent_id: Option<&LocalFileId>,
        nodes: &mut Vec<LocalNode>,
        inode_to_id: &mut HashMap<(u64, u64), LocalFileId>,
    ) -> Result<()> {
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let entry_path = entry.path();

            // Use symlink_metadata to skip symlinks/special files
            let Ok(metadata) = fs::symlink_metadata(&entry_path) else {
                continue;
            };

            // Skip symlinks and special files
            if metadata.file_type().is_symlink() || !(metadata.is_file() || metadata.is_dir()) {
                tracing::debug!(path = %entry_path.display(), "⏭️ Skipping non-regular file");
                continue;
            }

            let device_id = metadata.dev();
            let inode = metadata.ino();

            let id = inode_to_id
                .entry((device_id, inode))
                .or_insert_with(|| LocalFileId::new(device_id, inode))
                .clone();

            let name = entry.file_name().to_string_lossy().to_string();

            let (node_type, md5sum, size) = if metadata.is_dir() {
                (NodeType::Directory, None, None)
            } else {
                let size = metadata.len();
                let mtime = metadata.mtime();
                let md5sum = compute_md5_from_path(&entry_path)?;

                // TOCTOU protection: re-stat and verify unchanged
                let Ok(metadata_after) = fs::symlink_metadata(&entry_path) else {
                    continue; // File disappeared
                };
                if metadata_after.len() != size
                    || metadata_after.mtime() != mtime
                    || metadata_after.ino() != inode
                {
                    // File changed during hash, skip for now (will be caught on next scan)
                    tracing::debug!(path = %entry_path.display(), "⏭️ File changed during hash, skipping");
                    continue;
                }

                (NodeType::File, Some(md5sum), Some(size))
            };

            let node = LocalNode {
                id: id.clone(),
                parent_id: parent_id.cloned(),
                name,
                node_type,
                md5sum,
                size,
                mtime: metadata.mtime(),
            };

            nodes.push(node);

            if metadata.is_dir() {
                Self::scan_recursive(&entry_path, Some(&id), nodes, inode_to_id)?;
            }
        }

        Ok(())
    }

    /// Scan a single file or directory.
    ///
    /// # Errors
    /// Returns an error if filesystem operations fail.
    pub fn scan_file(path: &Path) -> Result<Option<LocalNode>> {
        Self::scan_file_with_retries(path, 3)
    }

    fn scan_file_with_retries(path: &Path, retries_left: u8) -> Result<Option<LocalNode>> {
        if !path.exists() {
            return Ok(None);
        }

        let metadata = fs::symlink_metadata(path)?;

        // Skip symlinks and special files
        if metadata.file_type().is_symlink() || !(metadata.is_file() || metadata.is_dir()) {
            return Ok(None);
        }

        let device_id = metadata.dev();
        let inode = metadata.ino();

        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();

        let (node_type, md5sum, size, mtime) = if metadata.is_dir() {
            (NodeType::Directory, None, None, metadata.mtime())
        } else {
            let size = metadata.len();
            let mtime = metadata.mtime();
            let md5sum = compute_md5_from_path(path)?;

            // TOCTOU protection: re-stat and verify unchanged
            let metadata_after = fs::symlink_metadata(path)?;
            if metadata_after.len() != size
                || metadata_after.mtime() != mtime
                || metadata_after.ino() != inode
            {
                // File changed while hashing, retry with bounded attempts
                if retries_left > 0 {
                    tracing::debug!(path = %path.display(), retries_left, "🔍 File changed during hash, retrying");
                    return Self::scan_file_with_retries(path, retries_left - 1);
                }
                // File still unstable after retries, skip it
                tracing::warn!(path = %path.display(), "⏭️ File unstable after retries, skipping");
                return Ok(None);
            }

            (NodeType::File, Some(md5sum), Some(size), mtime)
        };

        Ok(Some(LocalNode {
            id: LocalFileId::new(device_id, inode),
            parent_id: None, // Caller must set this
            name,
            node_type,
            md5sum,
            size,
            mtime,
        }))
    }
}
