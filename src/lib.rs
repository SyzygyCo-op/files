use worker::*;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
struct DriveShorcutDetails {
    #[serde(rename = "targetId")]
    target_id: String,
}

#[derive(Deserialize, Serialize)]
struct DriveFile {
    id: String,
    name: String,
    #[serde(rename = "mimeType")]
    mime_type: String,
    #[serde(rename = "webViewLink")]
    web_view_link: Option<String>,
    #[serde(rename = "webContentLink")]
    web_content_link: Option<String>,
    #[serde(rename = "shortcutDetails")]
    shortcut_details: Option<DriveShorcutDetails>,
}

#[derive(Deserialize)]
struct DriveResponse {
    files: Vec<DriveFile>,
}

#[event(fetch)]
async fn fetch(req: Request, env: Env, _ctx: Context) -> worker::Result<Response> {
    let url = req.url()?;
    let path = url.path();
    
    // Get API key and folder ID from environment variables
    let api_key = env.secret("GOOGLE_API_KEY")?.to_string();
    let folder_id = env.secret("GOOGLE_DRIVE_FOLDER_ID")?.to_string();
    
    match path {
        "/files/" => {
            // List files in the folder
            list_files(&api_key, &folder_id).await
        }
        path if path.starts_with("/files/") => {
            // Serve a specific file by name
            let file_name = &path[7..]; // Remove "/files/" prefix
            let decoded_name = urlencoding::decode(file_name)
                .map_err(|_| worker::Error::from("Invalid file name encoding"))?
                .to_string();
            serve_file_by_name(&api_key, &folder_id, &decoded_name).await
        }
        _ => Response::error("Not found", 404)
    }
}

async fn list_files(api_key: &str, folder_id: &str) -> worker::Result<Response> {
    let url = format!(
        "https://www.googleapis.com/drive/v3/files?q='{}'+in+parents&supportsAllDrives=true&includeItemsFromAllDrives=true&key={}",
        folder_id, api_key
    );
    
    let request = Request::new(&url, Method::Get)?;
    let mut response = Fetch::Request(request).send().await?;
    
    let status_code = response.status_code();
    if !(200..300).contains(&status_code) {
        return Response::error("Failed to fetch files from Google Drive", 500);
    }
    
    let drive_response: DriveResponse = response.json().await?;
    
    // Create a simple HTML page listing the files
    let mut html = String::from(r#"
<!DOCTYPE html>
<html>
<head>
    <title>Drive Files</title>
    <style>
        body { font-family: Arial, sans-serif; margin: 40px; }
        .file { margin: 10px 0; padding: 10px; border: 1px solid #ddd; border-radius: 5px; }
        .file-name { font-weight: bold; }
        .file-type { color: #666; font-size: 0.9em; }
        a { text-decoration: none; color: #1976d2; }
        a:hover { text-decoration: underline; }
    </style>
</head>
<body>
    <h1>Files in Drive Folder</h1>
"#);
    
    for file in drive_response.files {
        let encoded_name = urlencoding::encode(&file.name);
        html.push_str(&format!(
            r#"
    <div class="file">
        <div class="file-name">
            <a href="/files/{}">{}</a>
        </div>
        <div class="file-type">{}</div>
    </div>
"#,
            encoded_name, file.name, file.mime_type
        ));
    }
    
    html.push_str("</body></html>");
    
    Response::from_html(html)
}

async fn serve_file_by_name(api_key: &str, folder_id: &str, file_name: &str) -> worker::Result<Response> {
    // First, search for the file by name in the specified folder
    let search_url = format!(
        "https://www.googleapis.com/drive/v3/files?q=name='{}'+and+'{}'+in+parents&supportsAllDrives=true&includeItemsFromAllDrives=true&fields=files(id,name,mimeType,shortcutDetails)&key={}",
        file_name.replace("'", "\\'"), folder_id, api_key
    );
    
    let search_request = Request::new(&search_url, Method::Get)?;
    let mut search_response = Fetch::Request(search_request).send().await?;
    
    let search_status = search_response.status_code();
    if !(200..300).contains(&search_status) {
        return Response::error("Failed to search for file", 500);
    }
    
    let search_result: DriveResponse = search_response.json().await?;
    
    // Check if file was found
    if search_result.files.is_empty() {
        return Response::error("File not found", 404);
    }
    
    // Use the first matching file (in case of duplicates)
    let file_info = &search_result.files[0];

    if let Some(shortcut_details) = &file_info.shortcut_details {
        console_debug!("File is a shortcut, resolving target ID: {}", shortcut_details.target_id);
        // If it's a shortcut, we need to get the target file info
        let target_file_id = &shortcut_details.target_id;
        let target_url = format!(
            "https://www.googleapis.com/drive/v3/files/{}?supportsAllDrives=true&includeItemsFromAllDrives=true&key={}",
            target_file_id, api_key
        );
        
        let target_request = Request::new(&target_url, Method::Get)?;
        let mut target_response = Fetch::Request(target_request).send().await?;
        
        let target_status = target_response.status_code();
        if !(200..300).contains(&target_status) {
            return Response::error("Failed to fetch target file of shortcut", 500);
        }
        
        let target_file_info: DriveFile = target_response.json().await?;

        return serve_file_by_id(api_key, &target_file_info).await;
    } else {
        return serve_file_by_id(api_key, file_info).await;
    }
}

async fn serve_file_by_id(api_key: &str, file_info: &DriveFile) -> worker::Result<Response> {
    let file_id = &file_info.id;
    
    // Download the file content
    let download_url = format!(
        "https://www.googleapis.com/drive/v3/files/{}?alt=media&supportsAllDrives=true&includeItemsFromAllDrives=true&key={}",
        file_id, api_key
    );
    
    let download_request = Request::new(&download_url, Method::Get)?;
    let mut download_response = Fetch::Request(download_request).send().await?;
    
    let download_status = download_response.status_code();
    if !(200..300).contains(&download_status) {
        return Response::error("Failed to download file", 500);
    }
    
    // Create response with appropriate headers
    let headers = Headers::new();
    headers.set("Content-Type", &file_info.mime_type)?;
    headers.set("Content-Disposition", &format!("inline; filename=\"{}\"", file_info.name))?;
    
    let body = download_response.bytes().await?;
    
    Ok(Response::from_bytes(body)?.with_headers(headers))
}
