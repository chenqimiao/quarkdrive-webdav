use std::collections::HashMap;
use std::sync::{Arc};
use std::time::{Duration, SystemTime};
use model::*;

use reqwest_middleware::{ClientBuilder, ClientWithMiddleware};
use reqwest_retry::{Jitter, RetryTransientMiddleware};
use reqwest_retry::policies::ExponentialBackoff;
use anyhow::{Context, Result};
use serde::de::DeserializeOwned;
use serde::Serialize;
use tokio::time;
use tracing::{debug, error};

use base64::{Engine as _, engine::{self, general_purpose}, alphabet};


use reqwest::{
    header::{HeaderMap, HeaderValue},
    IntoUrl, StatusCode,
};

use dav_server::fs::{DavDirEntry, DavMetaData, FsFuture, FsResult};


use bytes::Bytes;
use dashmap::DashMap;
use headers::Cookie;
use moka::future::FutureExt;

pub mod model;

pub use model::{QuarkFile};

const ORIGIN: &str = "https://pan.quark.cn";
const REFERER: &str = "https://pan.quark.cn/";
const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) quark-cloud-drive/2.5.20 Chrome/100.0.4896.160 Electron/18.3.5.4-b478491100 Safari/537.36 Channel/pckk_other_ch";


#[derive(Debug, Clone)]
pub struct DriveConfig {
    pub api_base_url: String,
    pub cookie: Arc<DashMap<String, String>>,
}

#[derive(Debug, Clone)]
pub struct QuarkDrive {
    config: DriveConfig,
    client: ClientWithMiddleware,
    download_client: ClientWithMiddleware,
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
        let retry_policy = ExponentialBackoff::builder()
            .retry_bounds(Duration::from_millis(100), Duration::from_secs(5))
            .jitter(Jitter::Bounded)
            .base(2)
            .build_with_max_retries(5);

        let client = reqwest::Client::builder()
            .user_agent(UA)
            .default_headers(headers.clone())
            // Keep connections alive for better performance
            // OSS typically keeps connections open for 60+ seconds
            .pool_idle_timeout(Duration::from_secs(90))
            .connect_timeout(Duration::from_secs(10))
            .pool_max_idle_per_host(32) // Increase for concurrent operations
            .timeout(Duration::from_secs(300)) // Longer timeout for large file operations
            .build()?;
        let client = ClientBuilder::new(client)
            .with(RetryTransientMiddleware::new_with_policy(retry_policy))
            .build();
        
        // Configure download client with connection pooling for better performance
        let download_client = reqwest::Client::builder()
            .user_agent(UA)
            .default_headers(headers)
            // Enable connection pooling to avoid TCP handshake overhead on each request
            .pool_idle_timeout(Duration::from_secs(90))
            .pool_max_idle_per_host(32) // Increase pool size for concurrent downloads
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(300)) // Increase timeout for large files
            .build()?;
        let download_client = ClientBuilder::new(download_client)
            .with(RetryTransientMiddleware::new_with_policy(retry_policy))
            .build();

        let drive = Self {
            config,
            client,
            download_client,
        };


        Ok(drive)
    }

    async fn resolve_cookies(&self) -> String {
        self.config.cookie.iter()
            .map(|entry| format!("{}={}", entry.key(), entry.value()))
            .collect::<Vec<_>>()
            .join("; ")
    }

    async fn get_request<U>(&self, url: String, header: Option<HeaderMap>) -> Result<Option<U>>
    where
        U: DeserializeOwned,
    {
        let cookie = self.resolve_cookies().await;
        let url = reqwest::Url::parse(&url)?;
        let res = if let Some(headers) = header {
            self.client
                .get(url.clone())
                .headers(headers)
                .header("Cookie", cookie)
                .send()
                .await?
        } else {
            self.client
                .get(url.clone())
                .header("Cookie", cookie)
                .send()
                .await?
        };
        match res.error_for_status_ref() {
            Ok(_) => {
                if res.status() == StatusCode::NO_CONTENT {
                    return Ok(None);
                }
                self.update_cookie_from_response(&res).await;
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
                        | StatusCode::FORBIDDEN
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
    async fn update_cookie_from_response(&self, res: &reqwest::Response) {
        if let Some(set_cookie) = res.headers().get_all("set-cookie").iter().find_map(|v| v.to_str().ok()) {
            if let Some(puus) = set_cookie.split(';').find(|s| s.trim().starts_with("__puus=")) {
                let new_puus = puus.trim().to_string().replace("__puus=", "");
                self.config.cookie.insert("__puus".to_string(), new_puus);
            }
        }
    }
    
    async fn post_request<T, U>(&self, url: String, r: &T, headers: Option<HeaderMap> ) -> Result<Option<U>>
    where
        T: Serialize + ?Sized,
        U: DeserializeOwned,
    {
        let cookie = self.resolve_cookies().await;
        let url = reqwest::Url::parse(&url)?;
        let res = if let Some(headers) = headers {
            let is_xml = headers
                .get("Content-Type")
                .map(|v| v == "application/xml")
                .unwrap_or(false);
            if is_xml {
                let body = serde_json::to_value(r)?
                    .as_str()
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| serde_json::to_string(r).unwrap());
                self.client
                    .post(url.clone())
                    .body(body)
                    .headers(headers)
                    .header("Cookie", cookie)
                    .send()
                    .await?
            }else {
                self.client
                    .post(url.clone())
                    .json(r)
                    .headers(headers)
                    .header("Cookie", cookie)
                    .send()
                    .await?
            }

        } else {
            self.client
                .post(url.clone())
                .json(r)
                .header("Content-Type", "application/json")
                .header("Cookie", cookie)
                .send()
                .await?
        };



        match res.error_for_status_ref() {
            Ok(_) => {
                if res.status() == StatusCode::NO_CONTENT {
                    return Ok(None);
                }

                self.update_cookie_from_response(&res).await;

                let text = res.text().await?;
                debug!("{}: {}", url, text);
                // let res = serde_json::from_str(&res)?;
                let res = serde_json::from_str::<U>(&text)
                    .map_err(|e| anyhow::anyhow!("Failed to parse JSON response: {}", e))?;

                // let res = ;
                // let res = res.json::<U>().await?;
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
                        | StatusCode::FORBIDDEN
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
                        , size),
                None
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
                None
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
        let cookie = self.resolve_cookies().await;
        let url = url.into_url()?;
        let res = if let Some((start_pos, size)) = range {
            let end_pos = start_pos + size as u64 - 1;
            debug!(url = %url, start = start_pos, end = end_pos, "download file");
            let range = format!("bytes={}-{}", start_pos, end_pos);
            self.download_client
                .get(url)
                .header(RANGE, range)
                .header("Cookie", cookie)
                .send()
                .await?
                .error_for_status()?
        } else {
            debug!(url = %url, "download file");
            self.download_client.get(url).send().await?.error_for_status()?
        };
        self.update_cookie_from_response(&res).await;
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
                None
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
                None
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
                None
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
                None
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
                None)
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
            parallel_upload:false,
        };

        let res: UpPreResponse = self
            .post_request(
                format!("{}/1/clouddrive/file/upload/pre?pr=ucpro&fr=pc", self.config.api_base_url),
                &req,
                None
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
                &req,
                None
            )
            .await?
            .context("expect response")?;

        if res.status != 200 {
            return Err(anyhow::anyhow!("delete file failed: {}", res.message));
        }
        Ok(res)
    }

    pub async fn up_part_auth_meta(
        &self,
        mime_type: &str,
        utc_time: &str,
        bucket: &str,
        obj_key: &str,
        part_number: u32,
        upload_id: &str,
    ) -> Result<String> {
        let r = format!(
            "PUT\n\n{mime_type}\n{utc_time}\nx-oss-date:{utc_time}\nx-oss-user-agent:aliyun-sdk-js/6.6.1 Chrome 98.0.4758.80 on Windows 10 64-bit\n/{bucket}/{obj_key}?partNumber={part_number}&uploadId={upload_id}",
            mime_type = mime_type,
            utc_time = utc_time,
            bucket = bucket,
            obj_key = obj_key,
            part_number = part_number,
            upload_id = upload_id
        );
        Ok(r)
    }

    pub fn up_commit_auth_meta(
        &self,
        md5s: Vec<String>,
        callback: &Callback,
        bucket: &str,
        obj_key: &str,
        time_str: &str,
        upload_id: &str,
    ) -> Result<String> {
        // 构建XML内容
        let mut xml_body = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<CompleteMultipartUpload>\n");

        for (i, md5) in md5s.iter().enumerate() {
            xml_body.push_str(&format!(
                "<Part>\n<PartNumber>{}</PartNumber>\n<ETag>{}</ETag>\n</Part>\n",
                i + 1,
                md5
            ));
        }
        xml_body.push_str("</CompleteMultipartUpload>");

        // 计算XML内容的MD5
        let digest = md5::compute(xml_body.as_bytes());
        let content_md5 = general_purpose::STANDARD.encode(digest.0);
        // 序列化callback并Base64编码
        let callback_bytes = serde_json::to_vec(callback)?;
        let callback_base64 = general_purpose::STANDARD.encode(&callback_bytes);


        // 构建auth_meta字符串
        let auth_meta = format!(
            "POST\n{}\napplication/xml\n{}\nx-oss-callback:{}\nx-oss-date:{}\nx-oss-user-agent:aliyun-sdk-js/6.6.1 Chrome 98.0.4758.80 on Windows 10 64-bit\n/{}/{}?uploadId={}",
            content_md5,
            time_str,
            callback_base64,
            time_str,
            bucket,
            obj_key,
            upload_id
        );
        Ok(auth_meta)
    }
    pub async fn auth(&self, auth_info: &str, auth_meta: &str, task_id: &str) -> Result<AuthResponse> {

        let req = AuthRequest {
            auth_info: auth_info.to_string(),
            auth_meta: auth_meta.to_string(),
            task_id: task_id.to_string(),
        };

        let res: AuthResponse = self
            .post_request(
                format!("{}/1/clouddrive/file/upload/auth?pr=ucpro&fr=pc", self.config.api_base_url),
                &req,
                None
            )
            .await?
            .context("expect response")?;

        if res.status != 200 {
            return Err(anyhow::anyhow!("delete file failed: {}", res.message));
        }
        Ok(res)
    }

    pub async fn up_auth_and_commit(&self,
                                    req: UpAuthAndCommitRequest
    ) -> Result<()> {
        // 构建XML内容
        let mut xml_body = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<CompleteMultipartUpload>\n");

        for (i, md5) in req.md5s.iter().enumerate() {
            xml_body.push_str(&format!(
                "<Part>\n<PartNumber>{}</PartNumber>\n<ETag>{}</ETag>\n</Part>\n",
                i + 1,
                md5
            ));
        }
        xml_body.push_str("</CompleteMultipartUpload>");

        // 计算XML内容的MD5
        let digest = md5::compute(xml_body.as_bytes());

        let content_md5 = general_purpose::STANDARD.encode(digest.0);

        // 序列化callback并Base64编码
        let callback_bytes = serde_json::to_vec(&req.callback)?;
        let callback_base64 = general_purpose::STANDARD.encode(&callback_bytes);

        let now = chrono::Utc::now();
        let time_str = now.format("%a, %d %b %Y %H:%M:%S GMT").to_string();
        //let timestamp = now.timestamp_millis();

        // 构建auth_meta字符串
        let auth_meta = format!(
            "POST\n{}\napplication/xml\n{}\nx-oss-callback:{}\nx-oss-date:{}\nx-oss-user-agent:aliyun-sdk-js/6.6.1 Chrome 98.0.4758.80 on Windows 10 64-bit\n/{}/{}?uploadId={}",
            content_md5,
            &time_str,
            callback_base64,
            &time_str,
            req.bucket,
            req.obj_key,
            req.upload_id
        );
        let auth_key = self.auth(&req.auth_info, &auth_meta, &req.task_id).await
            .map_err(|e| {
                error!(error = %e, "Failed to authenticate and commit upload");
                e
            }).unwrap().data.auth_key;

        let commit_url = format!(
            "https://{}.{}/{}?uploadId={}",
            req.bucket,
            req.upload_url, // 去掉 https://
            req.obj_key,
            req.upload_id
        );

        let mut headers = HeaderMap::new();
        headers.insert("Authorization", HeaderValue::from_str(&auth_key)?);
        headers.insert("Content-MD5", HeaderValue::from_str(&content_md5)?);
        headers.insert("Content-Type", HeaderValue::from_str("application/xml")?);
        headers.insert("x-oss-callback", HeaderValue::from_str(&callback_base64)?);
        headers.insert("x-oss-date", HeaderValue::from_str(&time_str)?);
        headers.insert("x-oss-user-agent", HeaderValue::from_str("aliyun-sdk-js/6.6.1 Chrome 98.0.4758.80 on Windows 10 64-bit")?);
        headers.insert("Referer", HeaderValue::from_str(REFERER)?);

        //println!("{:#?}", headers);
        let _res: EmptyResponse = self.post_request(commit_url, &xml_body, Some(headers)) .await?.context("expect response")?;

        Ok(())



    }
    pub async fn finish(&self, obj_key: &str, task_id: &str) -> Result<FinishResponse> {

        let req = FinishRequest {
            obj_key: obj_key.to_string(),
            task_id: task_id.to_string(),
        };

        let res: FinishResponse = self
            .post_request(
                format!("{}/1/clouddrive/file/upload/finish?pr=ucpro&fr=pc", self.config.api_base_url),
                &req,
                None
            )
            .await?
            .context("expect response")?;

        if res.status != 200 {
            return Err(anyhow::anyhow!("delete file failed: {}", res.message));
        }
        // // sleep 500 sec for quark drive to process the finish request
        // time::sleep(Duration::from_secs(500)).await;
        Ok(res)
    }

    pub async fn up_part(&self, req: UpPartMethodRequest) -> Result<Option<String>> {
        let oss_url = format!(
            "https://{}.{}//{}?partNumber={}&uploadId={}",
            req.bucket,
            req.upload_url, // 去掉 https://
            req.obj_key,
            req.part_number,
            req.upload_id
        );
        let url = reqwest::Url::parse(&oss_url)?;
        let res = self
            .client
            .put(url.clone())
            .header("Authorization", req.auth_key.clone())
            .header("Content-Type", req.mime_type.clone())
            .header("x-oss-date", req.utc_time.clone())
            .header("x-oss-user-agent", "aliyun-sdk-js/6.6.1 Chrome 98.0.4758.80 on Windows 10 64-bit")
            .header("Referer", REFERER)
            .body(req.part_bytes)
            .send().await?;

        match res.error_for_status_ref() {
            Ok(_) => {
                if res.status() == StatusCode::NO_CONTENT {
                    return Ok(None);
                }
                let etag = res.headers().get("Etag").unwrap().to_str().unwrap();
                return Ok(Some(etag.to_string()));
            }
            Err(err) => {
                let err_msg = res.text().await?;
                debug!(error = %err_msg, url = %url, "request failed");
                match err.status() {
                    Some(
                        _status_code
                        @
                        (StatusCode::REQUEST_TIMEOUT
                        | StatusCode::TOO_MANY_REQUESTS
                        | StatusCode::INTERNAL_SERVER_ERROR
                        | StatusCode::BAD_GATEWAY
                        | StatusCode::SERVICE_UNAVAILABLE
                        | StatusCode::GATEWAY_TIMEOUT),
                    ) => {
                        time::sleep(Duration::from_secs(2)).await;
                        let res = self
                            .client
                            .put(url)
                            .send()
                            .await?
                            .error_for_status()?;
                        if res.status() == StatusCode::NO_CONTENT {
                            return Ok(None);
                        }
                        let etag = res.headers().get("Etag").unwrap().to_str().unwrap();
                        Ok(Some(etag.to_string()))
                    }
                    // unexpected error
                    _ => {
                        debug!(error = %err, "request failed");
                        Err(err.into())
                    }
                }
            }
        }
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
    use chrono::Utc;
    use super::*;

    #[tokio::test]
    async fn test_get_files_by_pdir_fid() {
        let cookie_str = std::env::var("QUARK_COOKIE").unwrap();
        let cookie = Arc::new(DashMap::new());
        for pair in cookie_str.split(';') {
            if let Some((k, v)) = pair.trim().split_once('=') {
                cookie.insert(k.trim().to_string(), v.trim().to_string());
            }
        }
        let config = DriveConfig {
            api_base_url: "https://drive.quark.cn".to_string(),
            cookie: cookie,
        };
        let drive = QuarkDrive::new(config).unwrap();
        let (files, _total) = drive.get_files_by_pdir_fid("0", 1, 50).await.unwrap();
        assert!(files.is_some());
        println!("{:?}", files);
    }


    #[tokio::test]
    async fn test_get_download_urls() {
        let cookie_str = std::env::var("QUARK_COOKIE").unwrap();
        let cookie = Arc::new(DashMap::new());
        for pair in cookie_str.split(';') {
            if let Some((k, v)) = pair.trim().split_once('=') {
                cookie.insert(k.trim().to_string(), v.trim().to_string());
            }
        }
        let config = DriveConfig {
            api_base_url: "https://drive.quark.cn".to_string(),
            cookie: cookie,
        };
        let drive = QuarkDrive::new(config).unwrap();
        let fids = vec!["your fid".to_string()];
        let res = drive.get_download_urls(fids).await.unwrap();
        assert!(!res.is_empty());
        println!("{:#?}", res);
    }
}
