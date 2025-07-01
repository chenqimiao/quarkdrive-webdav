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
    pub content_hash: Option<String>,
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


