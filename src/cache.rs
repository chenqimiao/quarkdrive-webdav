use std::path::Path;
use std::time::Duration;
use moka::future::Cache as MokaCache;
use tracing::debug;
use crate::drive::{DriveConfig, QuarkDrive};
use crate::drive::model::QuarkFile;

#[derive(Clone)]
pub struct Cache {
    inner: MokaCache<String, Vec<QuarkFile>>,
    drive: QuarkDrive,
}

const ONE_PAGE: u32 = 50;

impl Cache {
    pub fn new(max_capacity: u64, ttl: u64, drive: QuarkDrive) -> Self {
        let inner = MokaCache::builder()
            .max_capacity(max_capacity)
            .time_to_live(Duration::from_secs(ttl))
            .build();
        
        Self { inner , drive}
    }
    pub async fn get_or_insert(&self, key: &str) -> Option<Vec<QuarkFile>> {
        debug!(key = %key, "cache: get_or_insert");
        if let Some(files) = self.get(key).await {
            return Some(files);
        }
        if key == "/" {
            self.dfs(QuarkFile::new_root(), key, "/").await;
        }else {
            let mut path = Path::new(key);
            let mut cached_files:Vec<QuarkFile> = Vec::new();
            while let Some(parent) = path.parent() {
                if let Some(c_files) = self.get(parent.to_str().unwrap()).await {
                    cached_files = c_files;
                    break;
                }
                path = parent;
            }
            let dsf_root_file = cached_files.iter().filter(|quark_file| {
                quark_file.file_name == path.to_str().unwrap().split("/").last().unwrap()
            }).last().cloned().unwrap();
            self.dfs(dsf_root_file, key, path.to_str().unwrap()).await;
        }
        if let Some(files) = self.get(key).await {
            Some(files)
        }else {
            debug!(key = %key, "cache: no files found for key");
            None
        }
    }

    async fn dfs(&self, file: QuarkFile, target_path: &str, dfs_path: &str) {
        if file.dir {
            let mut current_files = Vec::<QuarkFile>::new();
            for page_no in 1..=204 {
                let (files, total) = self.drive.get_files_by_pdir_fid(&file.fid, page_no, ONE_PAGE).await.unwrap();
                let files = files.unwrap();
                let size = files.list.len();
                current_files.extend(files.list);
                if size < ONE_PAGE as usize || page_no >= total / ONE_PAGE + 1   {
                    break;
                }
            }

            self.insert(dfs_path.to_string(), current_files.clone()).await;
            debug!("{} in cache", &dfs_path);
            if dfs_path == target_path {
                return;
            }
            for curr_f in current_files {
                let file_path = if dfs_path == "/" {
                    format!("{}{}", dfs_path, curr_f.file_name)
                }else {
                    format!("{}/{}", dfs_path, curr_f.file_name)
                };
                if target_path.starts_with(&file_path) {
                    Box::pin(self.dfs(curr_f, target_path, &file_path)).await;
                }
            }

        }
    }

    async fn get(&self, key: &str) -> Option<Vec<QuarkFile>> {
        debug!(key = %key, "cache: get");
        self.inner.get(key).await
    }

    async fn insert(&self, key: String, value: Vec<QuarkFile>) {
        debug!(key = %key, "cache: insert");
        self.inner.insert(key, value).await;
    }

    pub async fn invalidate(&self, path: &Path) {
        let key = path.to_string_lossy().into_owned();
        debug!(path = %path.display(), key = %key, "cache: invalidate");
        self.inner.invalidate(&key).await;
    }

    pub async fn invalidate_parent(&self, path: &Path) {
        if let Some(parent) = path.parent() {
            self.invalidate(parent).await;
        }
    }

    pub fn invalidate_all(&self) {
        debug!("cache: invalidate all");
        self.inner.invalidate_all();
    }
}