use std::collections::HashMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Clone, Deserialize)]
pub struct QuarkFile {
    pub fid: String,
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
}


impl QuarkFile {
    pub fn new_root() -> Self {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64;
        Self {
            pdir_fid: "".to_string(),
            size: 0u64,
            format_type: "".to_string(),
            status: 1u8,
            created_at: now,
            updated_at: now,
            dir: true,
            file: false,
            file_name: "/".to_string(),
            fid: "0".to_string(),
            download_url: None,
        }
    }
}


#[derive(Debug, Serialize, Clone)]
pub struct GetFilesDownloadUrlsRequest {
    pub fids: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StreamInfo {
    pub size: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GetFileItem {
    pub fid: String,
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


pub type GetFilesResponse = Response<FilesData, FilesMetadata>;

pub type GetFilesDownloadUrlsResponse = Response<Vec<FileDownloadUrlItem>, FileDownloadUrlMetadata>;

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
pub struct QuarkFiles {
    pub list: Vec<QuarkFile>,
    pub total: u32,
}
#[derive(Debug, Clone, Deserialize)]
pub struct FileDownloadUrlItem {
    pub fid: String,
    pub download_url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FileDownloadUrlMetadata {
    
}

impl From<GetFilesResponse> for QuarkFiles {
    fn from(response: GetFilesResponse) -> Self {
        QuarkFiles {
            list: response.data.list,
            total: response.metadata.total,
        }
    }
}


