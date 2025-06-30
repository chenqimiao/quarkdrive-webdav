use std::collections::HashMap;
use std::time::{Duration, SystemTime};
use model::*;

use reqwest_middleware::{ClientBuilder, ClientWithMiddleware};
use reqwest_retry::{Jitter, RetryTransientMiddleware};
use reqwest_retry::policies::ExponentialBackoff;
use anyhow::{Context, Result};
use serde::de::DeserializeOwned;
use serde::Serialize;
use tokio::time;
use tracing::{debug};

use reqwest::{
    header::{HeaderMap, HeaderValue},
    IntoUrl, StatusCode,
};

use dav_server::fs::{DavDirEntry, DavMetaData, FsFuture, FsResult};


use bytes::Bytes;
use moka::future::FutureExt;

pub mod model;

pub use model::{QuarkFile};

const ORIGIN: &str = "https://pan.quark.cn";
const REFERER: &str = "https://pan.quark.cn/";
const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) quark-cloud-drive/2.5.20 Chrome/100.0.4896.160 Electron/18.3.5.4-b478491100 Safari/537.36 Channel/pckk_other_ch";


#[derive(Debug, Clone)]
pub struct DriveConfig {
    pub api_base_url: String,
    pub cookie: Option<String>,
}

#[derive(Debug, Clone)]
pub struct QuarkDrive {
    config: DriveConfig,
    client: ClientWithMiddleware,
}

impl DavMetaData for QuarkFile {
    fn len(&self) -> u64 {
        self.size
    }

    fn modified(&self) -> FsResult<SystemTime> {
        Ok(SystemTime::UNIX_EPOCH + Duration::from_millis(self.updated_at))
    }

    fn is_dir(&self) -> bool {
        self.dir
    }

    fn created(&self) -> FsResult<SystemTime> {
       Ok(SystemTime::UNIX_EPOCH + Duration::from_millis(self.created_at))
    }
}

impl DavDirEntry for QuarkFile {
    fn name(&self) -> Vec<u8> {
        self.file_name.as_bytes().to_vec()
    }

    fn metadata(&self) -> FsFuture<Box<dyn DavMetaData>> {
        async move { Ok(Box::new(self.clone()) as Box<dyn DavMetaData>) }.boxed()
    }
}


impl QuarkDrive {

    pub fn new(config: DriveConfig) -> Result<Self> {
        let mut headers = HeaderMap::new();
        headers.insert("Origin", HeaderValue::from_static(ORIGIN));
        headers.insert("Referer", HeaderValue::from_static(REFERER));
        let cookie = config.cookie.as_ref().expect("Please set QUARK_COOKIE in config!");
        headers.insert("Cookie", HeaderValue::from_str(cookie)?);
        let retry_policy = ExponentialBackoff::builder()
            .retry_bounds(Duration::from_secs(3), Duration::from_secs(7))
            .jitter(Jitter::Bounded)
            .base(2)
            .build_with_max_retries(3);
            
        let client = reqwest::Client::builder()
            .user_agent(UA)
            .default_headers(headers)
            // OSS closes idle connections after 60 seconds,
            // so we can close idle connections ahead of time to prevent re-using them.
            // See also https://github.com/hyperium/hyper/issues/2136
            .pool_idle_timeout(Duration::from_secs(50))
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .build()?;
        let client = ClientBuilder::new(client)
            .with(RetryTransientMiddleware::new_with_policy(retry_policy))
            .build();
        let drive = Self {
            config,
            client,
        };


        Ok(drive)
    }

    async fn get_request<U>(&self, url: String) -> Result<Option<U>>
    where
        U: DeserializeOwned,
    {
        let url = reqwest::Url::parse(&url)?;
        let res = self
            .client
            .get(url.clone())
            .send()
            .await?;
        match res.error_for_status_ref() {
            Ok(_) => {
                if res.status() == StatusCode::NO_CONTENT {
                    return Ok(None);
                }
                // let res = res.text().await?;
                // println!("{}: {}", url, res);
                // let res = serde_json::from_str(&res)?;
                let res = res.json::<U>().await?;
                Ok(Some(res))
            }
            Err(err) => {
                let err_msg = res.text().await?;
                debug!(error = %err_msg, url = %url, "request failed");
                match err.status() {
                    Some(
                        _status_code
                        @
                        // 4xx
                        ( StatusCode::REQUEST_TIMEOUT
                        | StatusCode::TOO_MANY_REQUESTS
                        // 5xx
                        | StatusCode::INTERNAL_SERVER_ERROR
                        | StatusCode::BAD_GATEWAY
                        | StatusCode::SERVICE_UNAVAILABLE
                        | StatusCode::GATEWAY_TIMEOUT),
                    ) => {
                        time::sleep(Duration::from_secs(1)).await;
                        let res = self
                            .client
                            .get(url.clone())
                            .send()
                            .await?;
                        if res.status() == StatusCode::NO_CONTENT {
                            return Ok(None);
                        }
                        let res = res.json::<U>().await?;
                        Ok(Some(res))
                    }
                    _ => Err(err.into()),
                }
            }
        }
    }


    async fn post_request<T, U>(&self, url: String, r: &T) -> Result<Option<U>>
    where
        T: Serialize + ?Sized,
        U: DeserializeOwned,
    {
        let url = reqwest::Url::parse(&url)?;
        let res= self
            .client
            .post(url.clone())
            .json(&r)
            .header("Content-Type", "application/json")
            .send()
            .await?;

        match res.error_for_status_ref() {
            Ok(_) => {
                if res.status() == StatusCode::NO_CONTENT {
                    return Ok(None);
                }
                // let res = res.text().await?;
                // println!("{}: {}", url, res);
                // let res = serde_json::from_str(&res)?;
                let res = res.json::<U>().await?;
                Ok(Some(res))
            }
            Err(err) => {
                let err_msg = res.text().await?;
                debug!(error = %err_msg, url = %url, "request failed");
                match err.status() {
                    Some(
                        _status_code
                        @
                        // 4xx
                        ( StatusCode::REQUEST_TIMEOUT
                        | StatusCode::TOO_MANY_REQUESTS
                        // 5xx
                        | StatusCode::INTERNAL_SERVER_ERROR
                        | StatusCode::BAD_GATEWAY
                        | StatusCode::SERVICE_UNAVAILABLE
                        | StatusCode::GATEWAY_TIMEOUT),
                    ) => {
                        time::sleep(Duration::from_secs(2)).await;
                        let res = self
                            .client
                            .post(url)
                            .send()
                            .await?
                            .error_for_status()?;
                        if res.status() == StatusCode::NO_CONTENT {
                            return Ok(None);
                        }
                        let res = res.json::<U>().await?;
                        Ok(Some(res))
                    }
                    _ => Err(err.into()),
                }
            }
        }
    }


    pub async fn get_files_by_pdir_fid(&self, pdir_fid: &str, page:u32, size:u32) -> Result<(Option<QuarkFiles>, u32)> {
        debug!(pdir_fid = %pdir_fid, page = %page, size = %size,  "get file");

        let res: Result<GetFilesResponse> = self
            .get_request(
                format!("{}/1/clouddrive/file/sort?pr=ucpro&fr=pc&&pdir_fid={}&_page={}&_size={}&_fetch_total=1&_fetch_sub_dirs=0&_sort=file_type:asc,updated_at:desc,"
                        , self.config.api_base_url
                        , pdir_fid
                        , page
                        , size)
            )
            .await
            .and_then(|res| res.context("unexpect response"));
        match res {
            Ok(files_res) =>{
                let total = files_res.metadata.total;
                Ok((Some(files_res.into()), total))
            },
            Err(err) => {
                if let Some(req_err) = err.downcast_ref::<reqwest::Error>() {
                    if matches!(req_err.status(), Some(StatusCode::NOT_FOUND)) {
                        Ok((None, 0u32))
                    } else {
                        Err(err)
                    }
                } else {
                    Err(err)
                }
            }
        }
    }

    pub async fn get_download_urls(&self, fids: Vec<String>) -> Result<HashMap<String, String>> {
        debug!(fids = ?fids, "get download url");
        let req = GetFilesDownloadUrlsRequest { fids };
        let res: GetFilesDownloadUrlsResponse = self
            .post_request(
                format!(
                    "{}/1/clouddrive/file/download?pr=ucpro&fr=pc",
                    self.config.api_base_url
                ),
                &req,
            )
            .await?
            .context("expect response")?;
        Ok(res.into_map())
    }

    pub async fn get_download_url(&self, fid: &str) -> Result<String> {
        debug!(fid = %fid, "get download url");
        self.get_download_urls(vec![fid.to_string()]).await?.iter().next()
            .map(|(_, url)| url.clone())
            .ok_or_else(|| anyhow::anyhow!("No download URL found for fid: {}", fid))
        
    }

    pub async fn download<U: IntoUrl>(&self, url: U, range: Option<(u64, usize)>) -> Result<Bytes> {
        use reqwest::header::RANGE;

        let url = url.into_url()?;
        let res = if let Some((start_pos, size)) = range {
            let end_pos = start_pos + size as u64 - 1;
            debug!(url = %url, start = start_pos, end = end_pos, "download file");
            let range = format!("bytes={}-{}", start_pos, end_pos);
            self.client
                .get(url)
                .header(RANGE, range)
                .send()
                .await?
                .error_for_status()?
        } else {
            debug!(url = %url, "download file");
            self.client.get(url).send().await?.error_for_status()?
        };
        Ok(res.bytes().await?)
    }

    pub async fn remove_file(&self, file_id: &str, trash: bool) -> Result<()> {
        // no untrash api in quark
        self.delete_file(file_id).await?;
        Ok(())
    }
    pub async fn rename_file(&self, file_id: &str, name: &str) -> Result<()> {
        debug!(file_id = %file_id, name = %name, "rename file");
        let req = RenameFileRequest {
            fid: file_id.to_string(),
            file_name: name.to_string(),
        };
        let res: RenameFileResponse = self
            .post_request(
                format!("{}/1/clouddrive/file/rename?pr=ucpro&fr=pc", self.config.api_base_url),
                &req,
            )
            .await?
            .context("expect response")?;
        if res.status != 200 {
            return Err(anyhow::anyhow!("delete file failed: {}", res.message));
        }
        Ok(())
    }


    pub async fn move_file(
        &self,
        file_id: &str,
        to_parent_file_id: &str,
    ) -> Result<()> {
        debug!(file_id = %file_id, to_parent_file_id = %to_parent_file_id, "move file");
        let req = MoveFileRequest {
            filelist: vec![file_id.to_string()],
            to_pdir_fid: to_parent_file_id.to_string(),
        };
        let res: CommonResponse = self
            .post_request(
                format!("{}/1/clouddrive/file/move?pr=ucpro&fr=pc", self.config.api_base_url),
                &req,
            )
            .await?
            .context("expect response")?;

        if res.status != 200 {
            return Err(anyhow::anyhow!("delete file failed: {}", res.message));
        }
        Ok(())
    }
    async fn delete_file(&self, file_id: &str) -> Result<()> {
        debug!(file_id = %file_id, "delete file");
        let req = DeleteFilesRequest {
            action_type: 2u8,
            exclude_fids: vec![],
            filelist: vec![file_id.to_string()],
        };
        let res: DeleteFilesResponse = self
            .post_request(
                format!(
                    "{}/1/clouddrive/file/delete?pr=ucpro&fr=pc",
                    self.config.api_base_url
                ),
                &req,
            )
            .await?
            .context("expect response")?;

        if res.status != 200 {
            return Err(anyhow::anyhow!("delete file failed: {}", res.message));
        }
        Ok(())
    }



    pub async fn create_folder(&self, parent_file_id: &str, name: &str) -> Result<()> {
        debug!(parent_file_id = %parent_file_id, name = %name, "create folder");
        let req = CreateFolderRequest {
            pdir_fid: parent_file_id.to_string(),
            file_name: name.to_string(),
            dir_path: "".to_string(),
            dir_init_lock: false,
        };
        let res: CreateFolderResponse = self
            .post_request(
                format!("{}/1/clouddrive/file?pr=ucpro&fr=pc", self.config.api_base_url),
                &req,
            )
            .await?
            .context("expect response")?;
        if res.status != 200 {
            return Err(anyhow::anyhow!("delete file failed: {}", res.message));
        }
        Ok(())
    }


    pub async fn get_quota(&self) -> Result<(u64, u64)> {
        let res: GetSpaceInfoResponse = self
            .get_request(
                format!("{}/1/clouddrive/member?pr=ucpro&fr=pc&uc_param_str=&fetch_subscribe=true&_ch=home&fetch_identity=true", self.config.api_base_url),
            )
            .await?
            .context("expect response")?;

        if res.status != 200 {
            return Err(anyhow::anyhow!("delete file failed: {}", res.message));
        }
        Ok((
            res.data.use_capacity,
            res.data.total_capacity,
        ))
    }

    pub async fn up_pre(&self, file_name: &str, size: u64, pdir_fid: &str) -> Result<UpPreResponse> {

        let format_type = get_format_type(file_name);

        let req = UpPreRequest {
            file_name: file_name.to_string(),
            size,
            pdir_fid: pdir_fid.to_string(),
            format_type: format_type.to_string(),
            ccp_hash_update: true,
            l_created_at: SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)?.as_millis() as u64,
            l_updated_at: SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)?.as_millis() as u64,
            // 上传文件夹？待确认
            dir_name: "".to_string(),
            parallel_upload:true,
        };

        let res: UpPreResponse = self
            .post_request(
                format!("{}/1/clouddrive/file/upload/pre?pr=ucpro&fr=pc", self.config.api_base_url),
                &req
            )
            .await?
            .context("expect response")?;

        if res.status != 200 {
            return Err(anyhow::anyhow!("delete file failed: {}", res.message));
        }
        Ok(res)
    }


    pub async fn up_hash(&self, md5: &str, sha1: &str, task_id: &str) -> Result<UpHashResponse> {

        

        let req = UpHashRequest {
            md5: md5.to_string(),
            sha1: sha1.to_string(),
            task_id: task_id.to_string(),
        };

        let res: UpHashResponse = self
            .post_request(
                format!("{}/1/clouddrive/file/update/hash?pr=ucpro&fr=pc", self.config.api_base_url),
                &req
            )
            .await?
            .context("expect response")?;

        if res.status != 200 {
            return Err(anyhow::anyhow!("delete file failed: {}", res.message));
        }
        Ok(res)
    }
}


fn get_format_type(file_name: &str) -> &str {
    if let Some(ext) = file_name.rsplit('.').next() {
        let ext = ext.to_lowercase();
        match ext.as_str() {
            "jpg" | "jpeg" => "image/jpeg",
            "png" => "image/png",
            "gif" => "image/gif",
            "mp4" => "video/mp4",
            "avi" => "video/x-msvideo",
            "mov" => "video/quicktime",
            "mp3" => "audio/mpeg",
            "wav" => "audio/wav",
            "pdf" => "application/pdf",
            "doc" | "docx" => "application/msword",
            "xls" | "xlsx" => "application/vnd.ms-excel",
            "ppt" | "pptx" => "application/vnd.ms-powerpoint",
            "txt" => "text/plain",
            "zip" => "application/zip",
            "rar" => "application/vnd.rar",
            "7z" => "application/x-7z-compressed",
            _ => "application/octet-stream",
        }
    } else {
        "application/octet-stream"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_get_files_by_pdir_fid() {
        let config = DriveConfig {
            api_base_url: "https://drive.quark.cn".to_string(),
            cookie: Some(std::env::var("QUARK_COOKIE").unwrap()),
        };
        let drive = QuarkDrive::new(config).unwrap();
        let (files, _total) = drive.get_files_by_pdir_fid("0", 1, 50).await.unwrap();
        assert!(files.is_some());
        println!("{:?}", files);
    }


    #[tokio::test]
    async fn test_get_download_urls() {
        let config = DriveConfig {
            api_base_url: "https://drive.quark.cn".to_string(),
            cookie: Some(std::env::var("QUARK_COOKIE").unwrap()),
        };
        let drive = QuarkDrive::new(config).unwrap();
        let fids = vec!["your fid".to_string()];
        let res = drive.get_download_urls(fids).await.unwrap();
        assert!(!res.is_empty());
        println!("{:#?}", res);
    }

    #[tokio::test]
    async fn test_download() {
        let config = DriveConfig {
            api_base_url: "https://drive.quark.cn".to_string(),
            cookie: Some(std::env::var("QUARK_COOKIE").unwrap()),
        };
        let drive = QuarkDrive::new(config).unwrap();
        let url = "";
        let bytes = drive.download(url, None).await.unwrap();
        assert!(!bytes.is_empty());
        println!("Downloaded {} bytes", bytes.len());
    }
}
