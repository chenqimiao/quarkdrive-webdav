use std::fmt::{Debug, Formatter};
use std::io::{SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use bytes::{Buf, Bytes, BytesMut};
use dashmap::DashMap;
use dav_server::{
    davpath::DavPath,
    fs::{
        DavDirEntry, DavFile, DavFileSystem, DavMetaData, FsError, FsFuture, FsStream, OpenOptions,
        ReadDirMeta,
    },
};
use futures_util::future::{ready, FutureExt};
use tracing::{debug, error, trace};
use crate::{
    cache::Cache,
    drive::{QuarkDrive, QuarkFile},
};
use bytes::BufMut;

use md5::Context as Md5Context;
use sha1::Sha1;
use tokio::io::AsyncWriteExt;

use sha1::Digest;
use tokio::fs::File;

use crate::drive::model::{Callback, UpAuthAndCommitRequest, UpPartMethodRequest};
use tokio::io::AsyncReadExt;

#[derive(Clone)]
pub struct QuarkDriveFileSystem {
    drive: QuarkDrive,
    pub(crate) dir_cache: Cache,
    uploading: Arc<DashMap<String, Vec<QuarkFile>>>,
    root: PathBuf,
    no_trash: bool,
    read_only: bool,
    upload_buffer_size: usize,
    skip_upload_same_size: bool,
    prefer_http_download: bool,
}

impl QuarkDriveFileSystem {
    #[allow(clippy::too_many_arguments)]
    pub fn new(drive: QuarkDrive, root: String, cache_size: u64, cache_ttl: u64) -> Result<Self> {
        let dir_cache = Cache::new(cache_size, cache_ttl, drive.clone());
        debug!("dir cache initialized");
        let root = if root.starts_with('/') {
            PathBuf::from(root)
        } else {
            Path::new("/").join(root)
        };
        Ok(Self {
            drive,
            dir_cache,
            uploading: Arc::new(DashMap::new()),
            root,
            no_trash: false,
            read_only: false,
            upload_buffer_size: 16 * 1024 * 1024,
            skip_upload_same_size: false,
            prefer_http_download: false,
        })
    }

    pub fn set_read_only(&mut self, read_only: bool) -> &mut Self {
        self.read_only = read_only;
        self
    }

    pub fn set_no_trash(&mut self, no_trash: bool) -> &mut Self {
        self.no_trash = no_trash;
        self
    }

    pub fn set_upload_buffer_size(&mut self, upload_buffer_size: usize) -> &mut Self {
        self.upload_buffer_size = upload_buffer_size;
        self
    }

    pub fn set_skip_upload_same_size(&mut self, skip_upload_same_size: bool) -> &mut Self {
        self.skip_upload_same_size = skip_upload_same_size;
        self
    }

    pub fn set_prefer_http_download(&mut self, prefer_http_download: bool) -> &mut Self {
        self.prefer_http_download = prefer_http_download;
        self
    }
    fn list_uploading_files(&self, parent_file_path: &str) -> Vec<QuarkFile> {
        self.uploading
            .get(parent_file_path)
            .map(|val_ref| val_ref.value().clone())
            .unwrap_or_default()
    }

    fn remove_uploading_file(&self, parent_file_path: &str, file_name: &str) {
        if let Some(mut files) = self.uploading.get_mut(parent_file_path) {
            if let Some(index) = files.iter().position(|x| x.file_name == file_name) {
                files.swap_remove(index);
            }
        }
    }

    async fn find_in_cache(&self, path: &Path) -> Result<Option<QuarkFile>, FsError> {
        if let Some(parent) = path.parent() {
            let parent_str = parent.to_string_lossy();
            let file_name = path
                .file_name()
                .ok_or(FsError::NotFound)?
                .to_string_lossy()
                .into_owned();
            let file = self.dir_cache.get_or_insert(&parent_str).await.and_then(|files| {
                for file in &files {
                    if file.file_name == file_name {
                        return Some(file.clone());
                    }
                }
                None
            });
            Ok(file)
        } else {
            let root = QuarkFile::new_root();
            Ok(Some(root))
        }
    }

    async fn get_file(&self, path: PathBuf) -> Result<Option<QuarkFile>, FsError> {
        let file = self.find_in_cache(&path).await?;
        if let Some(file) = file {
            trace!(path = %path.display(), file_id = %file.fid, "file found in cache");
            Ok(Some(file))
        } else {
            // find in drive
            Ok(None)
        }
    }


    fn normalize_dav_path(&self, dav_path: &DavPath) -> PathBuf {
        let path = dav_path.as_pathbuf();
        if self.root.parent().is_none() || path.starts_with(&self.root) {
            return path;
        }
        let rel_path = dav_path.as_rel_ospath();
        if rel_path == Path::new("") {
            return self.root.clone();
        }
        self.root.join(rel_path)
    }
}

impl DavFileSystem for QuarkDriveFileSystem {
    fn open<'a>(
        &'a self,
        dav_path: &'a DavPath,
        options: OpenOptions,
    ) -> FsFuture<'a, Box<dyn DavFile>> {
        let path = self.normalize_dav_path(dav_path);
        let mode = if options.write { "write" } else { "read" };
        debug!(path = %path.display(), mode = %mode, "fs: open");
        async move {
            if options.append {
                // Can't support open in write-append mode
                error!(path = %path.display(), "unsupported write-append mode");
                return Err(FsError::NotImplemented);
            }
            let parent_path = path.parent().ok_or(FsError::NotFound)?;
            let parent_file = self
                .get_file(parent_path.to_path_buf())
                .await?
                .ok_or(FsError::NotFound)?;
            let sha1 = options.checksum.and_then(|c| {
                if let Some((algo, hash)) = c.split_once(':') {
                    if algo.eq_ignore_ascii_case("sha1") {
                        Some(hash.to_string())
                    } else {
                        None
                    }
                } else {
                    None
                }
            });

            #[cfg(feature = "local_upload_hash")]
            if options.write && path.is_file() && sha1.is_none() {
                if let Ok((_, sha1_val)) = calc_md5_sha1(&path) {
                    sha1 = Some(sha1_val);
                }
            }
            let mut dav_file = if let Some(file) = self.get_file(path.clone()).await? {
                if options.write && options.create_new {
                    return Err(FsError::Exists);
                }
                if options.write && self.read_only {
                    return Err(FsError::Forbidden);
                }
                QuarkDavFile::new(
                    self.clone(),
                    file,
                    parent_file.fid,
                    parent_path.to_path_buf(),
                    options.size.unwrap_or_default(),
                    sha1,
                )
            } else if options.write && (options.create || options.create_new) {
                if self.read_only {
                    return Err(FsError::Forbidden);
                }

                let size = options.size;
                let name = dav_path
                    .file_name()
                    .ok_or(FsError::GeneralFailure)?
                    .to_string();

                // 忽略 macOS 上的一些特殊文件
                if name == ".DS_Store" || name.starts_with("._") {
                    return Err(FsError::NotFound);
                }

                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_millis();

                let file = QuarkFile {
                    fid: "".to_string(),
                    file_name: name,
                    pdir_fid: parent_file.fid.clone(),
                    size: size.unwrap_or(0),
                    format_type: "application/octet-stream".to_string(),
                    status: 1,
                    dir: false,
                    file: true,
                    content_hash: sha1.clone(),
                    created_at: now as u64,
                    updated_at: now as u64,
                    download_url: None,
                    parent_path: Some(parent_path.to_string_lossy().into_owned()),
                };

                let mut uploading = self.uploading.entry(parent_path.to_str().unwrap().to_string()).or_default();
                uploading.push(file.clone());
                QuarkDavFile::new(
                    self.clone(),
                    file,
                    parent_file.fid,
                    parent_path.to_path_buf(),
                    size.unwrap_or(0),
                    sha1,
                )
            } else {
                return Err(FsError::NotFound);
            };
            dav_file.http_download = self.prefer_http_download;
            Ok(Box::new(dav_file) as Box<dyn DavFile>)
        }
            .boxed()
    }

    fn read_dir<'a>(
        &'a self,
        path: &'a DavPath,
        _meta: ReadDirMeta,
    ) -> FsFuture<'a, FsStream<Box<dyn DavDirEntry>>> {
        let path = self.normalize_dav_path(path);
        debug!(path = %path.display(), "fs: read_dir");
        async move {
            let files = self.dir_cache.get_or_insert(&path.to_string_lossy())
                .await
                .ok_or(FsError::NotFound)
                .and_then(|files| {
                    Ok(files)
                })?;

            // 创建包含结果的向量
            let mut v: Vec<Result<Box<dyn DavDirEntry>, FsError>> = Vec::with_capacity(files.len());

            // 将每个文件转换为 trait 对象
            for file in files {
                v.push(Ok(Box::new(file))); // 现在类型匹配了
            }

            // 创建流并装箱
            let stream = futures_util::stream::iter(v);
            Ok(Box::pin(stream) as FsStream<Box<dyn DavDirEntry>>)
        }
            .boxed()
    }

    fn metadata<'a>(&'a self, path: &'a DavPath) -> FsFuture<'a, Box<dyn DavMetaData>> {
        let mut path = self.normalize_dav_path(path);
        if path.as_path().to_str() == Some("0") {
            // root path
            debug!("fs: metadata for root");
            path = PathBuf::from("/");
        }
        debug!(path = %path.display(), "fs: metadata");
        async move {
            // if root return
            if path == self.root {
                debug!("fs: metadata for root");
                let root_file = QuarkFile::new_root();
                return Ok(Box::new(root_file) as Box<dyn DavMetaData>);
            }
            // if not found in cache, get from uploading files: self.fs.uploading
            let mut file = self.get_file(path.clone()).await.unwrap_or_else(|_| Option::None);
            if file.is_none() {
                let parent_path = path.parent().ok_or(FsError::NotFound)?;
                file = self.list_uploading_files(parent_path.to_str().unwrap())
                    .first().cloned();

            };

            let file = file.ok_or(FsError::NotFound)?;

            Ok(Box::new(file) as Box<dyn DavMetaData>)
        }
            .boxed()
    }
    fn have_props<'a>(
        &'a self,
        _path: &'a DavPath,
    ) -> std::pin::Pin<Box<dyn futures_util::Future<Output = bool> + Send + 'a>> {
        Box::pin(ready(true))
    }

    fn get_prop(&self, dav_path: &DavPath, prop: dav_server::fs::DavProp) -> FsFuture<Vec<u8>> {
        let path = self.normalize_dav_path(dav_path);
        let prop_name = match prop.prefix.as_ref() {
            Some(prefix) => format!("{}:{}", prefix, prop.name),
            None => prop.name.to_string(),
        };
        debug!(path = %path.display(), prop = %prop_name, "fs: get_prop");
        async move {
            if prop.namespace.as_deref() == Some("http://owncloud.org/ns")
                && prop.name == "checksums"
            {
                let file = self.get_file(path).await?.ok_or(FsError::NotFound)?;
                if let Some(sha1) = file.content_hash {
                    let xml = format!(
                        r#"<?xml version="1.0"?>
                        <oc:checksums xmlns:d="DAV:" xmlns:nc="http://nextcloud.org/ns" xmlns:oc="http://owncloud.org/ns">
                            <oc:checksum>sha1:{}</oc:checksum>
                        </oc:checksums>
                    "#,
                        sha1
                    );
                    return Ok(xml.into_bytes());
                }
            }
            Err(FsError::NotImplemented)
        }
            .boxed()
    }

    fn get_quota(&self) -> FsFuture<(u64, Option<u64>)> {
        debug!("fs: get_quota");
        async move {
            let (used, total) = self.drive.get_quota().await.map_err(|err| {
                error!(error = %err, "get quota failed");
                FsError::GeneralFailure
            })?;
            Ok((used, Some(total)))
        }
            .boxed()
    }

    fn create_dir<'a>(&'a self, dav_path: &'a DavPath) -> FsFuture<'a, ()> {
        let path = self.normalize_dav_path(dav_path);
        debug!(path = %path.display(), "fs: create_dir");
        async move {
            if self.read_only {
                return Err(FsError::Forbidden);
            }

            let files = self.dir_cache.get_or_insert(&path.to_string_lossy())
                .await;
            if files.is_some() {
                return Ok(());
            }
            let parent_path = path.parent().ok_or(FsError::NotFound)?;
            let parent_file = self
                .get_file(parent_path.to_path_buf())
                .await?
                .ok_or(FsError::NotFound)?;
            if !parent_file.dir {
                return Err(FsError::Forbidden);
            }
            if let Some(name) = path.file_name() {
                let name = name.to_string_lossy().into_owned();
                self.drive
                    .create_folder(&parent_file.fid, &name)
                    .await
                    .map_err(|err| {
                        error!(path = %path.display(), error = %err, "create folder failed");
                        FsError::GeneralFailure
                    })?;
                // sleep 1s for quark server to update cache
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                self.dir_cache.invalidate(parent_path).await;
                Ok(())
            } else {
                Err(FsError::Forbidden)
            }
        }
            .boxed()
    }


    fn remove_dir<'a>(&'a self, dav_path: &'a DavPath) -> FsFuture<'a, ()> {
        let path = self.normalize_dav_path(dav_path);
        debug!(path = %path.display(), "fs: remove_dir");
        async move {
            if self.read_only {
                return Err(FsError::Forbidden);
            }

            let file = self
                .get_file(path.clone())
                .await?
                .ok_or(FsError::NotFound)?;
            if !file.dir {
                return Err(FsError::Forbidden);
            }
            self.drive
                .remove_file(&file.fid, !self.no_trash)
                .await
                .map_err(|err| {
                    error!(path = %path.display(), error = %err, "remove directory failed");
                    FsError::GeneralFailure
                })?;
            self.dir_cache.invalidate(&path).await;
            self.dir_cache.invalidate_parent(&path).await;
            Ok(())
        }
            .boxed()
    }

    fn remove_file<'a>(&'a self, dav_path: &'a DavPath) -> FsFuture<'a, ()> {
        let path = self.normalize_dav_path(dav_path);
        debug!(path = %path.display(), "fs: remove_file");
        async move {
            if self.read_only {
                return Err(FsError::Forbidden);
            }

            let file = self
                .get_file(path.clone())
                .await?
                .ok_or(FsError::NotFound)?;
            if !file.file {
                return Err(FsError::Forbidden);
            }
            self.drive
                .remove_file(&file.fid, !self.no_trash)
                .await
                .map_err(|err| {
                    error!(path = %path.display(), error = %err, "remove file failed");
                    FsError::GeneralFailure
                })?;
            // sleep 500ms for quark server to update cache
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            self.dir_cache.invalidate_parent(&path).await;
            Ok(())
        }
            .boxed()
    }

    fn copy<'a>(&'a self, from_dav: &'a DavPath, to_dav: &'a DavPath) -> FsFuture<'a, ()> {
        // not support by quark api
        async move {
            Err(FsError::NotImplemented)
        }.boxed()
    }

    fn rename<'a>(&'a self, from_dav: &'a DavPath, to_dav: &'a DavPath) -> FsFuture<'a, ()> {
        let from = self.normalize_dav_path(from_dav);
        let to = self.normalize_dav_path(to_dav);
        debug!(from = %from.display(), to = %to.display(), "fs: rename");
        async move {
            if self.read_only {
                return Err(FsError::Forbidden);
            }

            let is_dir;
            if from.parent() == to.parent() {
                // rename
                if let Some(name) = to.file_name() {
                    let file = self
                        .get_file(from.clone())
                        .await?
                        .ok_or(FsError::NotFound)?;
                    is_dir = file.dir;
                    let name = name.to_string_lossy().into_owned();
                    self.drive
                        .rename_file(&file.fid, &name)
                        .await
                        .map_err(|err| {
                            error!(from = %from.display(), to = %to.display(), error = %err, "rename file failed");
                            FsError::GeneralFailure
                        })?;
                } else {
                    return Err(FsError::Forbidden);
                }
            } else {
                // move
                let file = self
                    .get_file(from.clone())
                    .await?
                    .ok_or(FsError::NotFound)?;
                is_dir = file.dir;
                let to_parent_file = self
                    .get_file(to.parent().unwrap().to_path_buf())
                    .await?
                    .ok_or(FsError::NotFound)?;
                let new_name = to_dav.file_name();
                self.drive
                    .move_file(&file.fid, &to_parent_file.fid)
                    // then rename ...
                    .await
                    .map_err(|err| {
                        error!(from = %from.display(), to = %to.display(), error = %err, "move file failed");
                        FsError::GeneralFailure
                    })?;
                if let Some(to_name) = new_name {
                    if let Some(from_name) = from_dav.file_name(){
                        if from_name != to_name {
                            self.drive.rename_file(&file.fid, to_name)
                                .await
                                .map_err(|err| {
                                    error!(from = %from.display(), to = %to.display(), error = %err, "rename file after move failed");
                                    FsError::GeneralFailure
                                })?;
                        }
                    }
                }
            }

            if is_dir {
                self.dir_cache.invalidate(&from).await;
            }
            self.dir_cache.invalidate_parent(&from).await;
            self.dir_cache.invalidate_parent(&to).await;
            Ok(())
        }
            .boxed()
    }

}

#[derive(Debug, Clone)]
struct UploadState {
    size: u64,
    buffer: BytesMut,
    chunk_count: u64,
    chunk_size: u64,
    chunk: u64,
    upload_id: String,
    upload_url: String,
    sha1: Option<String>,
    task_id: String,
    temp_file_path: String,
    is_finished: bool,
    bucket: String,
    obj_key: String,
    mime_type: String,
    auth_info: String,
    callback: Option<Callback>,
    is_uploading: bool,

}

impl Default for UploadState {
    fn default() -> Self {
        Self {
            size: 0,
            buffer: BytesMut::new(),
            chunk_count: 0,
            chunk_size: 0,
            chunk: 1,
            upload_id: String::new(),
            upload_url: "".to_string(),
            sha1: None,
            task_id: "".to_string(),
            temp_file_path: "".to_string(),
            is_finished: false,
            bucket: "".to_string(),
            obj_key: "".to_string(),
            mime_type: "application/octet-stream".to_string(),
            auth_info: "".to_string(),
            callback: None,
            is_uploading: false,
        }
    }
}

struct QuarkDavFile {
    fs: QuarkDriveFileSystem,
    file: QuarkFile,
    parent_file_id: String,
    parent_dir: PathBuf,
    current_pos: u64,
    upload_state: UploadState,
    http_download: bool,
    md5_ctx: Md5Context,
    sha1_ctx: Sha1,
}

impl Debug for QuarkDavFile {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QuarkDavFile")
            .field("file", &self.file)
            .field("parent_file_id", &self.parent_file_id)
            .field("current_pos", &self.current_pos)
            .field("upload_state", &self.upload_state)
            .finish()
    }
}

impl QuarkDavFile {

    fn new(
        fs: QuarkDriveFileSystem,
        file: QuarkFile,
        parent_file_id: String,
        parent_dir: PathBuf,
        size: u64,
        sha1: Option<String>,
    ) -> Self {
        Self {
            fs,
            file,
            parent_file_id,
            parent_dir,
            current_pos: 0,
            upload_state: UploadState {
                size,
                sha1,
                ..Default::default()
            },
            http_download: false,
            md5_ctx: Md5Context::new(),
            sha1_ctx: Sha1::default(),
        }
    }
    async fn prepare_for_upload(&mut self) -> Result<bool, FsError> {
        if self.upload_state.is_finished  {
            return Ok(false);
        }
        if !self.upload_state.is_uploading {
           self.upload_state.is_uploading = true;
        }
        if self.upload_state.chunk_count == 0 {
            let size = self.upload_state.size;
            debug!(file_name = %self.file.file_name, size = size, "prepare for upload");
            if !self.file.fid.is_empty() {
                if let Some(content_hash) = self.file.content_hash.as_ref() {
                    if let Some(sha1) = self.upload_state.sha1.as_ref() {
                        if content_hash.eq_ignore_ascii_case(sha1) {
                            debug!(file_name = %self.file.file_name, sha1 = %sha1, "skip uploading same content hash file");
                            return Ok(false);
                        }
                    }
                }
                if self.fs.skip_upload_same_size && self.file.size == size {
                    debug!(file_name = %self.file.file_name, size = size, "skip uploading same size file");
                    self.upload_state.is_finished = true;
                    return Ok(false);
                }
                // existing file, delete before upload
                if let Err(err) = self
                    .fs
                    .drive
                    .remove_file(&self.file.fid, !self.fs.no_trash)
                    .await
                {
                    error!(file_name = %self.file.file_name, error = %err, "delete file before upload failed");
                }
            }
            // 由云端计算
            // 除了 size 和sha1 其余由云端获得
            // TODO ..
            // let upload_buffer_size = self.fs.upload_buffer_size as u64;
            // let chunk_count =
            //     size / upload_buffer_size + if size % upload_buffer_size != 0 { 1 } else { 0 };
            // self.upload_state.chunk_count = chunk_count;
        }
        Ok(true)

    }
    async fn consume_buf(&mut self) -> Result<(), FsError> {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis();
        self.upload_state.temp_file_path = format!("./temp/{}_{}", timestamp, self.file.file_name);
        let temp_path = self.upload_state.temp_file_path.clone();
        let mut md5_ctx = self.md5_ctx.clone();
        let mut sha1_ctx = self.sha1_ctx.clone();
        let bytes = self.upload_state.buffer.split().freeze().to_vec();
        // 写入临时文件
        self.upload_state.size = self.upload_state.size + bytes.len() as u64;
        if let Some(parent) = std::path::Path::new(&temp_path).parent() {
            tokio::fs::create_dir_all(parent).await.ok();
        }
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .append(true)
            .open(&temp_path)
            .await
            .map_err(|e| {
                error!("failed to open file: {}, {}", temp_path, e);
                FsError::GeneralFailure
            })?;
        file.write_all(&bytes).await.map_err(|e| {
            error!(file_name = %self.file.file_name, error = %e, "write to temp file failed");
            FsError::GeneralFailure
        })?;
        file.flush().await.map_err(|e| {
            error!(file_name = %self.file.file_name, error = %e, "flush temp file failed");
            FsError::GeneralFailure
        })?;
        // 更新哈希
        md5_ctx.consume(&bytes);
        sha1_ctx.update(&bytes);
        // 保存回结构体
        self.md5_ctx = md5_ctx;
        self.sha1_ctx = sha1_ctx;
        Ok(())
    }

    async fn upload_chunk(&mut self) -> Result<(), FsError> {

        let chunk_size = self.upload_state.chunk_size as usize;
        let temp_path = &self.upload_state.temp_file_path;
        let file = File::open(temp_path).await.map_err(|err| {
            error!(file_name = %self.file.file_name, error = %err, "open temp file failed");
            FsError::GeneralFailure
        })?;
        let mut file = tokio::io::BufReader::new(file);
        let mut buffer = BytesMut::with_capacity(chunk_size);
        let chunk_count = self.upload_state.chunk_count;
        // 定义一个字符串数组，size = chunk_count
        let mut etags = vec![String::new(); chunk_count as usize];
        // 分块上传文件,将temp_path目录所在文件,切成chunk_count块，每块大小 chunk_size，分块上传文件到夸克网盘
        // auth
        let mime_type = &self.upload_state.mime_type;
        let obj_key = &self.upload_state.obj_key;
        let bucket = &self.upload_state.bucket;
        let task_id = &self.upload_state.task_id;
        let upload_id = &self.upload_state.upload_id;
        let upload_url = &self.upload_state.upload_url;

        for chunk_idx in 1..= chunk_count {
            buffer.clear();
            let bytes_read = file.read_buf(&mut buffer).await.map_err(|err| {
                error!(file_name = %self.file.file_name, error = %err, "read temp file failed");
                FsError::GeneralFailure
            })?;
            if bytes_read == 0 {
                break; // EOF
            }
            if buffer.len() > chunk_size {
                buffer.truncate(chunk_size);
            }

            let now: chrono::DateTime<chrono::Utc> = chrono::Utc::now();
            // RFC1123 格式
            let utc_time = now.format("%a, %d %b %Y %H:%M:%S GMT").to_string();
            let auth_meta = self.fs.drive.up_part_auth_meta(mime_type, &utc_time, bucket, obj_key, chunk_idx as u32, upload_id).await.map_err(|err| {
                error!(file_name = %self.file.file_name, error = %err, "get upload part auth meta failed");
                FsError::GeneralFailure
            })?;
            let auth_info = &self.upload_state.auth_info;

            let auth_res = self.fs.drive.auth(auth_info, &auth_meta, task_id).await.map_err(|err| {
                error!(file_name = %self.file.file_name, error = %err, "auth upload part failed");
                FsError::GeneralFailure
            })?;


            let auth_key = auth_res.data.auth_key;

            let up_req = UpPartMethodRequest {
                auth_key: auth_key.clone(),
                mime_type: self.upload_state.mime_type.clone(),
                utc_time: utc_time.clone(),
                bucket: bucket.clone(),
                upload_url: upload_url.clone(),
                obj_key: obj_key.clone(),
                part_number: chunk_idx as u32,
                upload_id: upload_id.to_string(),
                part_bytes: buffer.to_vec(),
            };

            let res = self.fs.drive.up_part(up_req).await.map_err(|err| {
                error!(file_name = %self.file.file_name, error = %err, "upload chunk failed");
                FsError::GeneralFailure
            })?;
            let etag_from_up_part = res.unwrap();
            // 检查是否提前完成
            if etag_from_up_part == "finish" {
                return Ok(());
            }
            etags[(chunk_idx - 1) as usize] = etag_from_up_part;
            // self.upload_state.chunk += 1;
        }
        let callback = self.upload_state.callback.clone().unwrap();

        let auth_info = &self.upload_state.auth_info;
        let commit_req = UpAuthAndCommitRequest{
            md5s: etags.clone(),
            callback: callback,
            bucket: bucket.clone(),
            obj_key: obj_key.clone(),
            upload_id: upload_id.clone(),
            auth_info: auth_info.clone(),
            task_id: task_id.clone(),
            upload_url: upload_url.clone(),
        };
        // commit
        self.fs.drive.up_auth_and_commit(commit_req).await.map_err(|err| {
            error!(file_name = %self.file.file_name, error = %err, "commit upload failed");
            FsError::GeneralFailure
        })?;
        // finish upload
        self.fs.drive.finish(&obj_key, &task_id).await.map_err(|err| {
            error!(file_name = %self.file.file_name, error = %err, "finish upload failed");
            FsError::GeneralFailure
        })?;

        Ok(())
    }

    async fn delete_temp_file(&self) -> Result<(), FsError> {
        let temp_path = &self.upload_state.temp_file_path;
        if tokio::fs::metadata(temp_path).await.is_ok() {
            if let Err(err) = tokio::fs::remove_file(temp_path).await {
                error!(file_id = %self.file.fid, file_name = %self.file.file_name, error = %err, "remove temp file failed");
            }
        }
        Ok(())
    }

    async fn after_flush(&mut self) -> Result<(), FsError> {
        self.delete_temp_file().await?;
        let parent_path = self.file.parent_path.as_ref().unwrap().as_str();
        self.fs.remove_uploading_file(parent_path, &self.file.file_name);
        self.upload_state = UploadState::default();
        // sleep 500ms for quark server to update cache
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        self.fs.dir_cache.invalidate(self.parent_dir.as_path()).await;
        Ok(())
    }

    async fn get_download_url(&self) -> Result<String, FsError> {
        self.fs.drive.get_download_url(&self.file.fid).await.map_err(|err| {
            error!(file_id = %self.file.fid, file_name = %self.file.file_name, error = %err, "get download url failed");
            FsError::GeneralFailure
        })
    }

}

impl DavFile for QuarkDavFile {
    fn metadata(&'_ mut self) -> FsFuture<'_, Box<dyn DavMetaData>> {
        debug!(file_id = %self.file.fid, file_name = %self.file.file_name, "file: metadata");
        async move {
            let file = self.file.clone();
            Ok(Box::new(file) as Box<dyn DavMetaData>)
        }
            .boxed()
    }

    fn redirect_url(&mut self) -> FsFuture<Option<String>> {
        debug!(file_id = %self.file.fid, file_name = %self.file.file_name, "file: redirect_url");
        async move {
            if self.file.fid.is_empty() {
                return Err(FsError::NotFound);
            }
            let download_url = self.fs.drive.get_download_url(&self.file.fid).await.unwrap();

            return Ok(Some(download_url));

        }
            .boxed()
    }



    fn seek(&mut self, pos: SeekFrom) -> FsFuture<u64> {
        debug!(
            file_id = %self.file.fid,
            file_name = %self.file.file_name,
            pos = ?pos,
            "file: seek"
        );
        async move {
            let new_pos = match pos {
                SeekFrom::Start(pos) => pos,
                SeekFrom::End(pos) => (self.file.size as i64 + pos) as u64,
                SeekFrom::Current(size) => self.current_pos + size as u64,
            };
            self.current_pos = new_pos;
            Ok(new_pos)
        }
            .boxed()
    }

    /// write file : open -> metadata -> flush -> write_buf/write_byte -> flush
    fn write_buf(&mut self, buf: Box<dyn bytes::Buf + Send>) -> FsFuture<()>{
        debug!(file_id = %self.file.fid, file_name = %self.file.file_name, "file: write_buf");
        async move {
            if self.prepare_for_upload().await? {
                self.upload_state.buffer.put(buf);
                self.consume_buf().await?;
            }
            Ok(())
        }
            .boxed()
    }


    fn write_bytes(&mut self, buf: bytes::Bytes) -> FsFuture<()> {
        let buf: Box<dyn Buf + Send> = Box::new(buf);
        self.write_buf(buf)
    }

    fn read_bytes(&mut self, count: usize) -> FsFuture<Bytes> {
        debug!(
            file_id = %self.file.fid,
            file_name = %self.file.file_name,
            pos = self.current_pos,
            count = count,
            size = self.file.size,
            "file: read_bytes",
        );
        async move {
            if self.file.fid.is_empty() {
                // upload in progress
                return Err(FsError::NotFound);
            }
            // 检查现有 URL 是否有效
            let is_valid = self.file.download_url.as_ref()
                .map(|url| !is_url_expired(url))
                .unwrap_or(false);

            if !is_valid {
                let new_url = self.get_download_url().await.unwrap();
                self.file.download_url = Some(new_url);
            }
            let download_url = match self.file.download_url.as_ref() {
                Some(url) => url,
                None => {
                    // 详细记录文件信息
                    println!(
                        "文件缺少下载URL: {:?}\n文件元数据: {:#?}",
                        self.file.download_url,
                        self.file);
                    return Err(dav_server::fs::FsError::NotFound);
                }
            };

            if !download_url.is_empty() {
                let content = self.fs.drive.download(download_url, Some((self.current_pos, count))).await.unwrap();
                self.current_pos += content.len() as u64;
                return Ok(content);
            }else {
                return Err(FsError::NotFound);
            }
        }
            .boxed()
    }

    fn flush(&mut self) -> FsFuture<()> {
        debug!(file_id = %self.file.fid, file_name = %self.file.file_name, "file: flush");
        async move {
            if !self.upload_state.is_uploading {
                debug!(file_id = %self.file.fid, file_name = %self.file.file_name, "file: flush - no temp file path");
                return Ok(());
            }

            if self.upload_state.is_finished {
                debug!(file_id = %self.file.fid, file_name = %self.file.file_name, "file: flush - already finished");
                return Ok(());
            }
            let size = self.upload_state.size;

            // up_pre
            let res = self
                .fs
                .drive
                .up_pre(&self.file.file_name, size, &self.parent_file_id)
                .await
                .map_err(|err| {
                    error!(file_name = %self.file.file_name, error = %err, "create file with proof failed");
                    FsError::GeneralFailure
                })?;

            if res.data.finish {
                // 秒传
                self.upload_state.is_finished = true;
                self.after_flush().await?;
                return Ok(());
            }
            self.upload_state.auth_info = res.data.auth_info;
            self.upload_state.callback = Some(res.data.callback.clone());
            self.upload_state.task_id = res.data.task_id.clone();
            self.upload_state.upload_url =
                res.data.upload_url
                    .strip_prefix("https://")
                    .or_else(|| res.data.upload_url.strip_prefix("http://"))
                    .unwrap_or(&res.data.upload_url)
                    .to_string();
            self.upload_state.bucket = res.data.bucket;
            self.upload_state.obj_key = res.data.obj_key;
            if res.data.format_type != "" {
                self.upload_state.mime_type = res.data.format_type;
            }

            self.file.fid = res.data.fid.clone();

            self.upload_state.chunk_size = res.metadata.part_size;
            let chunk_count =
                size / res.metadata.part_size + if size % res.metadata.part_size != 0 { 1 } else { 0 };
            self.upload_state.chunk_count = chunk_count;
            let Some(upload_id) = res.data.upload_id else {
                error!("create file with proof failed: missing upload_id");
                return Err(FsError::GeneralFailure);
            };
            self.upload_state.upload_id = upload_id;

            // unHash
            let md5 = self.md5_ctx.clone().compute();
            let md5 = format!("{:x}", md5);
            let sha1 = self.sha1_ctx.clone().finalize();
            let sha1 = format!("{:x}", sha1);
            let task_id = self.upload_state.task_id.clone();
            let res = self.fs.drive.up_hash(&md5, &sha1, &task_id).await.map_err(|err| {
                error!(file_id = %self.file.fid, file_name = %self.file.file_name, error = %err, "hash file failed");
                FsError::GeneralFailure
            })?;
            if res.data.finish {
                self.upload_state.is_finished = true;
                self.after_flush().await?;
                return Ok(());
            }
            self.upload_chunk().await?;
            self.after_flush().await?;
            Ok(())
        }.boxed()

    }
}



fn is_url_expired(url: &str) -> bool {
    if let Ok(oss_url) = ::url::Url::parse(url) {
        let expires = oss_url.query_pairs().find_map(|(k, v)| {
            if k == "Expires" {
                if let Ok(expires) = v.parse::<u64>() {
                    return Some(expires);
                }
            }
            None
        });
        if let Some(expires) = expires {
            let current_ts = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("Time went backwards")
                .as_secs();
            // 预留 1 分钟
            return current_ts >= expires - 60;
        }
    }
    false
}