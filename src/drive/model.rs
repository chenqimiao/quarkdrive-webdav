use std::collections::HashMap;

use serde::{Deserialize, Serialize, Deserializer};

/// Custom deserializer for file_name that HTML-unescapes the value
fn deserialize_file_name<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    let s = String::deserialize(deserializer)?;
    Ok(htmlescape::decode_html(&s).unwrap_or(s))
}

#[derive(Debug, Serialize, Clone, Deserialize)]
pub struct QuarkFile {
    pub fid: String,
    #[serde(deserialize_with = "deserialize_file_name")]
    pub file_name: String,
    pub pdir_fid: String,
    #[serde(default)]
    pub size: u64,
    pub format_type: String,
    pub status: u8,
    pub created_at: u64,
    pub updated_at: u64,
    pub dir: bool,
    pub file: bool,
    pub download_url:Option<String>,
    pub content_hash: Option<String>,
    pub parent_path: Option<String>,
}


impl QuarkFile {
    pub fn new_root() -> Self {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64;
        Self {
            pdir_fid: "".to_string(),
            size: 0u64,
            format_type: "".to_string(),
            parent_path: None,
            status: 1u8,
            created_at: now,
            updated_at: now,
            dir: true,
            file: false,
            file_name: "".to_string(),
            fid: "0".to_string(),
            download_url: None,
            content_hash: None,
        }
    }
}


#[derive(Debug, Serialize, Clone)]
pub struct GetFilesDownloadUrlsRequest {
    pub fids: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GetFileItem {
    pub fid: String,
    #[serde(deserialize_with = "deserialize_file_name")]
    pub file_name: String,
    pub pdir_fid: String,
    pub category: u8,
    pub file_type: u8,
    #[serde(default)]
    pub size: u64,
    pub format_type: String,
    pub status: u8,
    pub tag: String,
    pub created_at: u64,
    pub updated_at: u64,
    pub dir: bool,
    pub file: bool,
}


#[derive(Debug, Serialize, Clone)]
pub struct DeleteFilesRequest {
    pub action_type: u8,
    pub exclude_fids: Vec<String>,
    pub filelist: Vec<String>,
}


#[derive(Debug, Serialize, Clone)]
pub struct CreateFolderRequest {
    pub pdir_fid: String,
    pub file_name: String,
    pub dir_path: String,
    pub dir_init_lock: bool,
}

#[derive(Debug, Serialize, Clone)]
pub struct RenameFileRequest {
    pub fid: String,
    pub file_name: String,
}

#[derive(Debug, Serialize, Clone)]
pub struct MoveFileRequest {
    pub filelist: Vec<String>,
    pub to_pdir_fid: String,
}

#[derive(Debug, Serialize, Clone)]
pub struct UpPreRequest {
    pub file_name: String,
    pub size: u64,
    pub pdir_fid: String,
    pub format_type: String,
    pub ccp_hash_update: bool,
    pub l_created_at: u64,
    pub l_updated_at: u64,
    pub parallel_upload: bool,
    pub dir_name: String,
}

#[derive(Debug, Serialize, Clone)]
pub struct UpHashRequest {
    pub md5: String,
    pub sha1: String,
    pub task_id: String,
}


#[derive(Debug, Serialize, Clone)]
pub struct AuthRequest {
    pub auth_info: String,
    pub auth_meta: String,
    pub task_id: String,
}

#[derive(Debug, Serialize, Clone)]
pub struct FinishRequest {
    pub obj_key: String,
    pub task_id: String,
}


pub struct UpPartMethodRequest {
    pub auth_key: String,
    pub mime_type: String,
    pub utc_time: String,
    pub bucket: String,
    pub upload_url: String,
    pub obj_key: String,
    pub part_number: u32,
    pub upload_id: String,
    pub part_bytes: Vec<u8>,
}


pub type GetFilesResponse = Response<FilesData, FilesMetadata>;

pub type GetFilesDownloadUrlsResponse = Response<Vec<FileDownloadUrlItem>, FileDownloadUrlMetadata>;

pub type DeleteFilesResponse = Response<DeleteFilesData, DeleteFilesMetadata>;

pub type CreateFolderResponse = Response<CreateFolderData, EmptyMetadata>;

pub type RenameFileResponse = Response<EmptyData, EmptyMetadata>;

pub type CommonResponse = Response<EmptyData, EmptyMetadata>;

pub type GetSpaceInfoResponse = Response<GetSpaceInfoResponseData, EmptyMetadata>;
pub type UpPreResponse = Response<UpPreResponseData, UpPreResponseMetaData>;

pub type UpHashResponse = Response<UpHashResponseData, EmptyMetadata>;

pub type AuthResponse = Response<AuthResponseData, EmptyMetadata>;

pub type FinishResponse = Response<EmptyData, EmptyMetadata>;


impl GetFilesDownloadUrlsResponse {
    pub fn into_map(self) -> HashMap<String, String> {
        self.data.into_iter().map(|item| (item.fid, item.download_url)).collect()
    }
}
#[derive(Debug, Clone, Deserialize)]
pub struct Response<T, U> {
    pub status: u8,
    pub code: u32,
    pub message: String,
    pub timestamp: u64,
    pub data: T,
    pub metadata: U,
}


#[derive(Debug, Clone, Deserialize)]
pub struct EmptyResponse {

}



#[derive(Debug, Clone, Deserialize)]
pub struct FilesData {
    pub list: Vec<QuarkFile>,

}

#[derive(Debug, Clone, Deserialize)]
pub struct FilesMetadata {
    #[serde(rename = "_total")]
    pub total: u32,
    #[serde(rename = "_count")]
    pub count: u32,
    #[serde(rename = "_page")]
    pub page: u32,

}

#[derive(Debug, Clone, Deserialize)]
pub struct DeleteFilesData {
    pub task_id: String,
    pub finish: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DeleteFilesMetadata {
    pub tq_gap: u32,
}


#[derive(Debug, Clone, Deserialize)]
pub struct CreateFolderData {
    pub finish: bool,
    pub fid: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EmptyMetadata {

}

#[derive(Debug, Clone, Deserialize)]
pub struct EmptyData {

}


#[derive(Debug, Clone, Deserialize)]
pub struct QuarkFiles {
    pub list: Vec<QuarkFile>,
    pub total: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FileDownloadUrlItem {
    pub fid: String,
    pub download_url: String,
    #[serde(default)]
    pub md5: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FileDownloadUrlMetadata {

}
#[derive(Debug, Clone, Deserialize)]
pub struct GetSpaceInfoResponseData {
    pub total_capacity: u64,
    pub use_capacity: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GetSpaceInfoResponseMetaData {

}

#[derive(Debug, Clone, Deserialize)]
pub struct AuthResponseData {
    pub auth_key: String,
}


#[derive(Debug, Clone, Deserialize)]
pub struct UpPreResponseData {
    pub finish: bool,
    pub task_id: String,
    pub upload_id: Option<String>,
    pub auth_info: String,
    pub upload_url: String,
    pub obj_key: String,
    pub fid: String,
    pub bucket: String,
    pub format_type: String,
    pub auth_info_expried: u64,
    pub callback: Callback,

}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpAuthAndCommitRequest {
    pub md5s: Vec<String>,
    pub callback: Callback,
    pub bucket: String,
    pub obj_key: String,
    pub upload_id: String,
    pub auth_info: String,
    pub task_id: String,
    pub upload_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Callback {
    #[serde(rename = "callbackUrl")]
    pub callback_url: String,
    #[serde(rename = "callbackBody")]
    pub callback_body: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UpPreResponseMetaData {
    pub part_size: u64,
    pub part_thread: u32
}

#[derive(Debug, Clone, Deserialize)]
pub struct UpHashResponseData {
    pub finish: bool,
}




impl From<GetFilesResponse> for QuarkFiles {
    fn from(response: GetFilesResponse) -> Self {
        QuarkFiles {
            list: response.data.list,
            total: response.metadata.total,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_quark_file_new_root() {
        let root = QuarkFile::new_root();
        assert_eq!(root.fid, "0");
        assert!(root.dir);
        assert!(!root.file);
        assert_eq!(root.size, 0);
        assert_eq!(root.pdir_fid, "");
        assert_eq!(root.file_name, "");
        assert!(root.created_at > 0);
        assert!(root.updated_at > 0);
    }

    #[test]
    fn test_quark_file_deserialize_basic() {
        let json = r#"{
            "fid": "abc123",
            "file_name": "test.txt",
            "pdir_fid": "0",
            "size": 1024,
            "format_type": "text/plain",
            "status": 1,
            "created_at": 1704067200000,
            "updated_at": 1704067200000,
            "dir": false,
            "file": true,
            "download_url": null,
            "content_hash": null,
            "parent_path": null
        }"#;
        let file: QuarkFile = serde_json::from_str(json).unwrap();
        assert_eq!(file.fid, "abc123");
        assert_eq!(file.file_name, "test.txt");
        assert_eq!(file.size, 1024);
        assert!(!file.dir);
        assert!(file.file);
    }

    #[test]
    fn test_quark_file_deserialize_html_entity() {
        let json = r#"{
            "fid": "abc123",
            "file_name": "test &amp; file &lt;1&gt;.txt",
            "pdir_fid": "0",
            "size": 0,
            "format_type": "",
            "status": 1,
            "created_at": 0,
            "updated_at": 0,
            "dir": false,
            "file": true,
            "download_url": null,
            "content_hash": null,
            "parent_path": null
        }"#;
        let file: QuarkFile = serde_json::from_str(json).unwrap();
        assert_eq!(file.file_name, "test & file <1>.txt");
    }

    #[test]
    fn test_get_files_download_urls_response_into_map() {
        let response = GetFilesDownloadUrlsResponse {
            status: 200,
            code: 0,
            message: "ok".to_string(),
            timestamp: 1704067200,
            data: vec![
                FileDownloadUrlItem {
                    fid: "fid1".to_string(),
                    download_url: "https://example.com/1".to_string(),
                    md5: None,
                },
                FileDownloadUrlItem {
                    fid: "fid2".to_string(),
                    download_url: "https://example.com/2".to_string(),
                    md5: None,
                },
            ],
            metadata: FileDownloadUrlMetadata {},
        };
        let map = response.into_map();
        assert_eq!(map.len(), 2);
        assert_eq!(map.get("fid1").unwrap(), "https://example.com/1");
        assert_eq!(map.get("fid2").unwrap(), "https://example.com/2");
    }

    #[test]
    fn test_get_files_download_urls_response_into_map_empty() {
        let response = GetFilesDownloadUrlsResponse {
            status: 200,
            code: 0,
            message: "ok".to_string(),
            timestamp: 0,
            data: vec![],
            metadata: FileDownloadUrlMetadata {},
        };
        let map = response.into_map();
        assert!(map.is_empty());
    }

    // --- QuarkFile deserialization edge cases ---

    #[test]
    fn test_quark_file_deserialize_with_optional_fields() {
        let json = r#"{
            "fid": "abc",
            "file_name": "test.txt",
            "pdir_fid": "0",
            "size": 2048,
            "format_type": "text/plain",
            "status": 1,
            "created_at": 1704067200000,
            "updated_at": 1704067200000,
            "dir": false,
            "file": true,
            "download_url": "https://example.com/download",
            "content_hash": "abc123sha1hash",
            "parent_path": "/docs"
        }"#;
        let file: QuarkFile = serde_json::from_str(json).unwrap();
        assert_eq!(file.download_url, Some("https://example.com/download".to_string()));
        assert_eq!(file.content_hash, Some("abc123sha1hash".to_string()));
        assert_eq!(file.parent_path, Some("/docs".to_string()));
    }

    #[test]
    fn test_quark_file_deserialize_size_default() {
        // size field uses #[serde(default)], so missing size should default to 0
        let json = r#"{
            "fid": "abc",
            "file_name": "dir",
            "pdir_fid": "0",
            "format_type": "",
            "status": 1,
            "created_at": 0,
            "updated_at": 0,
            "dir": true,
            "file": false,
            "download_url": null,
            "content_hash": null,
            "parent_path": null
        }"#;
        let file: QuarkFile = serde_json::from_str(json).unwrap();
        assert_eq!(file.size, 0);
        assert!(file.dir);
    }

    // --- FileDownloadUrlItem deserialization ---

    #[test]
    fn test_file_download_url_item_with_md5() {
        let json = r#"{
            "fid": "file_123",
            "download_url": "https://cdn.example.com/file",
            "md5": "d41d8cd98f00b204e9800998ecf8427e"
        }"#;
        let item: FileDownloadUrlItem = serde_json::from_str(json).unwrap();
        assert_eq!(item.fid, "file_123");
        assert_eq!(item.download_url, "https://cdn.example.com/file");
        assert_eq!(item.md5, Some("d41d8cd98f00b204e9800998ecf8427e".to_string()));
    }

    #[test]
    fn test_file_download_url_item_without_md5() {
        // md5 uses #[serde(default)], so missing field should be None
        let json = r#"{
            "fid": "file_123",
            "download_url": "https://cdn.example.com/file"
        }"#;
        let item: FileDownloadUrlItem = serde_json::from_str(json).unwrap();
        assert_eq!(item.fid, "file_123");
        assert!(item.md5.is_none());
    }

    #[test]
    fn test_file_download_url_item_null_md5() {
        let json = r#"{
            "fid": "file_123",
            "download_url": "https://cdn.example.com/file",
            "md5": null
        }"#;
        let item: FileDownloadUrlItem = serde_json::from_str(json).unwrap();
        assert!(item.md5.is_none());
    }

    // --- Response structure ---

    #[test]
    fn test_response_structure_deserialize() {
        let json = r#"{
            "status": 200,
            "code": 0,
            "message": "ok",
            "timestamp": 1704067200,
            "data": {},
            "metadata": {}
        }"#;
        let resp: Response<EmptyData, EmptyMetadata> = serde_json::from_str(json).unwrap();
        assert_eq!(resp.status, 200);
        assert_eq!(resp.code, 0);
        assert_eq!(resp.message, "ok");
        assert_eq!(resp.timestamp, 1704067200);
    }

    #[test]
    fn test_response_error_status() {
        // status is u8 in the API (200 = success, other values = error)
        let json = r#"{
            "status": 0,
            "code": 10001,
            "message": "invalid params",
            "timestamp": 1704067200,
            "data": {},
            "metadata": {}
        }"#;
        let resp: Response<EmptyData, EmptyMetadata> = serde_json::from_str(json).unwrap();
        assert_ne!(resp.status, 200);
        assert_eq!(resp.code, 10001);
        assert_eq!(resp.message, "invalid params");
    }

    // --- Callback serialization ---

    #[test]
    fn test_callback_serialize_camel_case() {
        let callback = Callback {
            callback_url: "https://example.com/callback".to_string(),
            callback_body: "test_body".to_string(),
        };
        let json = serde_json::to_string(&callback).unwrap();
        // Should use camelCase due to #[serde(rename)]
        assert!(json.contains("callbackUrl"));
        assert!(json.contains("callbackBody"));
        assert!(!json.contains("callback_url"));
        assert!(!json.contains("callback_body"));
    }

    #[test]
    fn test_callback_deserialize_camel_case() {
        let json = r#"{
            "callbackUrl": "https://example.com/cb",
            "callbackBody": "body_content"
        }"#;
        let cb: Callback = serde_json::from_str(json).unwrap();
        assert_eq!(cb.callback_url, "https://example.com/cb");
        assert_eq!(cb.callback_body, "body_content");
    }

    #[test]
    fn test_callback_roundtrip() {
        let original = Callback {
            callback_url: "https://example.com/callback".to_string(),
            callback_body: "{\"key\":\"value\"}".to_string(),
        };
        let json = serde_json::to_string(&original).unwrap();
        let deserialized: Callback = serde_json::from_str(&json).unwrap();
        assert_eq!(original.callback_url, deserialized.callback_url);
        assert_eq!(original.callback_body, deserialized.callback_body);
    }

    // --- FilesMetadata deserialization ---

    #[test]
    fn test_files_metadata_deserialize() {
        let json = r#"{
            "_total": 100,
            "_count": 50,
            "_page": 1
        }"#;
        let meta: FilesMetadata = serde_json::from_str(json).unwrap();
        assert_eq!(meta.total, 100);
        assert_eq!(meta.count, 50);
        assert_eq!(meta.page, 1);
    }

    // --- UpHashResponseData ---

    #[test]
    fn test_up_hash_response_finish_true() {
        let json = r#"{"finish": true}"#;
        let data: UpHashResponseData = serde_json::from_str(json).unwrap();
        assert!(data.finish);
    }

    #[test]
    fn test_up_hash_response_finish_false() {
        let json = r#"{"finish": false}"#;
        let data: UpHashResponseData = serde_json::from_str(json).unwrap();
        assert!(!data.finish);
    }

    // --- GetSpaceInfoResponseData ---

    #[test]
    fn test_space_info_deserialize() {
        let json = r#"{
            "total_capacity": 6597069766656,
            "use_capacity": 1234567890
        }"#;
        let data: GetSpaceInfoResponseData = serde_json::from_str(json).unwrap();
        assert_eq!(data.total_capacity, 6597069766656);
        assert_eq!(data.use_capacity, 1234567890);
    }

    // --- QuarkFiles from GetFilesResponse ---

    #[test]
    fn test_quark_files_from_response() {
        let response = GetFilesResponse {
            status: 200,
            code: 0,
            message: "ok".to_string(),
            timestamp: 0,
            data: FilesData {
                list: vec![
                    QuarkFile::new_root(),
                ],
            },
            metadata: FilesMetadata {
                total: 1,
                count: 1,
                page: 1,
            },
        };
        let quark_files: QuarkFiles = response.into();
        assert_eq!(quark_files.total, 1);
        assert_eq!(quark_files.list.len(), 1);
        assert_eq!(quark_files.list[0].fid, "0");
    }
}


