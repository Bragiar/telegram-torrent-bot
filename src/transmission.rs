use hyper::header::AUTHORIZATION;
use hyper::{client, Body, Request, Response};
use serde::Deserialize;
use serde_json::json;
use std::env;

use crate::jackett::TorrentLocation;

fn transmission_path(env: String) -> Result<String, String> {
    env::var(env).map_err(|_| {
        "TRANSMISSION_TV_PATH or TRANSMISSION_MOVIE_PATH env var is not set".to_string()
    })
}

fn transmission_url() -> String {
    env::var("TRANSMISSION_URL").map_or("http://localhost:9091".to_string(), |url| url)
}

#[derive(Clone, Debug, PartialEq)]
pub enum Media {
    TV,
    Movie,
}

#[derive(Debug, Deserialize)]
struct TransmissionResponse {
    result: String,
    arguments: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Torrent {
    pub id: i64,
    pub name: String,
    pub status: i64,
    #[serde(rename = "percentDone")]
    pub percent_done: f64,
    #[serde(rename = "downloadDir")]
    pub download_dir: String,
    #[serde(rename = "totalSize")]
    pub total_size: i64,
    #[serde(rename = "downloadedEver")]
    pub downloaded_ever: i64,
    #[serde(rename = "uploadedEver")]
    pub uploaded_ever: i64,
    #[allow(dead_code)]
    #[serde(rename = "seedRatioLimit")]
    pub seed_ratio_limit: f64,
    #[allow(dead_code)]
    #[serde(rename = "seedIdleLimit")]
    pub seed_idle_limit: i64,
}

fn transmission_credentials() -> Option<String> {
    env::var("TRANSMISSION_CREDENTIALS").ok()
}

async fn request_transmission_rpc(
    client: &client::Client<hyper_rustls::HttpsConnector<client::HttpConnector>>,
    method: &str,
    arguments: serde_json::Value,
    token: Option<String>,
) -> hyper::Result<Response<Body>> {
    let creds = transmission_credentials();

    let mut builder = Request::builder()
        .uri(format!("{}/transmission/rpc", transmission_url()))
        .method("POST");

    let headers = builder.headers_mut().unwrap();
    if let Some(creds) = creds {
        let basic = base64::encode(creds);
        let header = format!("Basic {}", basic).parse().unwrap();
        headers.insert(AUTHORIZATION, header);
    }

    if let Some(token) = token {
        headers.insert("X-Transmission-Session-Id", token.parse().unwrap());
    }

    let json = json!({
        "method": method,
        "arguments": arguments
    });

    let body = json.to_string().into();
    let request = builder.body(body).unwrap();

    client.request(request).await
}

async fn request_transmission(
    client: &client::Client<hyper_rustls::HttpsConnector<client::HttpConnector>>,
    location: TorrentLocation,
    path: String,
    token: Option<String>,
) -> hyper::Result<Response<Body>> {
    let arguments = if location.is_magnet {
        json!({
            "download-dir": path,
            "filename": location.content,
        })
    } else {
        json!({
            "download-dir": path,
            "metainfo": location.content,
        })
    };

    request_transmission_rpc(client, "torrent-add", arguments, token).await
}

async fn request_transmission_with_retry(
    client: &client::Client<hyper_rustls::HttpsConnector<client::HttpConnector>>,
    method: &str,
    arguments: serde_json::Value,
) -> Result<Response<Body>, String> {
    let transmission_response =
        request_transmission_rpc(client, method, arguments.clone(), None).await;

    if transmission_response.is_err() {
        return Err("Transmission replied with error".to_string());
    }

    let response = transmission_response.unwrap();
    if response.status() == 409 {
        let headers = response.headers();
        let header_value = headers.get("X-Transmission-Session-Id");
        if header_value.is_none() {
            return Err("First request to transmission didn't bring the token".to_string());
        }

        let session_value = header_value.unwrap().to_str().unwrap().to_string();
        let retry_response = request_transmission_rpc(client, method, arguments, Some(session_value))
            .await
            .map_err(|e| format!("Error on retry: {:?}", e))?;

        if retry_response.status().is_success() {
            Ok(retry_response)
        } else {
            Err(format!("Error on transmission {}", retry_response.status()))
        }
    } else if response.status().is_success() {
        Ok(response)
    } else {
        Err(format!("Error on transmission {}", response.status()))
    }
}

async fn request_add_torrent(location: TorrentLocation, path: String) -> Result<(), String> {
    let https = hyper_rustls::HttpsConnector::with_native_roots();
    let client: client::Client<_> = client::Client::builder().build(https);

    let transmission_response =
        request_transmission(&client, location.clone(), path.clone(), None).await;

    if transmission_response.is_err() {
        return Err("Transmission replied with error".to_string());
    }

    let response = transmission_response.unwrap();
    if response.status() == 409 {
        let headers = response.headers();
        let header_value = headers.get("X-Transmission-Session-Id");
        if header_value.is_none() {
            return Err("First request to transmission didn't bring the token {}".to_string());
        }

        let session_value = header_value.unwrap().to_str().unwrap().to_string();
        request_transmission(&client, location.clone(), path.clone(), Some(session_value))
            .await
            .unwrap();
        Ok(())
    } else {
        Err(format!("Error on transmission {}", response.status()))
    }
}

pub async fn add_torrent(location: TorrentLocation, media: Media) -> Result<(), String> {
    let path = match media {
        Media::TV => transmission_path("TRANSMISSION_TV_PATH".to_string())?,
        Media::Movie => transmission_path("TRANSMISSION_MOVIE_PATH".to_string())?,
    };

    request_add_torrent(location, path.clone()).await?;
    Ok(())
}

pub async fn get_torrents() -> Result<Vec<Torrent>, String> {
    let https = hyper_rustls::HttpsConnector::with_native_roots();
    let client: client::Client<_> = client::Client::builder().build(https);

    let arguments = json!({
        "fields": [
            "id", "name", "status", "percentDone", "downloadDir",
            "totalSize", "downloadedEver", "uploadedEver",
            "seedRatioLimit", "seedIdleLimit"
        ]
    });

    let response = request_transmission_with_retry(&client, "torrent-get", arguments).await?;

    let body_bytes = hyper::body::to_bytes(response.into_body())
        .await
        .map_err(|e| format!("Failed to read response body: {}", e))?;

    let transmission_response: TransmissionResponse = serde_json::from_slice(&body_bytes)
        .map_err(|e| format!("Failed to parse Transmission response: {}", e))?;

    if transmission_response.result != "success" {
        return Err(format!("Transmission error: {}", transmission_response.result));
    }

    if let Some(args) = transmission_response.arguments {
        if let Some(torrents_array) = args.get("torrents") {
            let torrents: Vec<Torrent> = serde_json::from_value(torrents_array.clone())
                .map_err(|e| format!("Failed to parse torrents: {}", e))?;
            return Ok(torrents);
        }
    }

    Ok(Vec::new())
}

pub async fn delete_torrent(ids: Vec<i64>) -> Result<(), String> {
    let https = hyper_rustls::HttpsConnector::with_native_roots();
    let client: client::Client<_> = client::Client::builder().build(https);

    let arguments = json!({
        "ids": ids,
        "delete-local-data": false
    });

    request_transmission_with_retry(&client, "torrent-remove", arguments).await?;
    Ok(())
}

pub async fn stop_seeding_all() -> Result<(), String> {
    let https = hyper_rustls::HttpsConnector::with_native_roots();
    let client: client::Client<_> = client::Client::builder().build(https);

    // First get all torrents
    let torrents = get_torrents().await?;
    
    if torrents.is_empty() {
        return Ok(());
    }

    let ids: Vec<i64> = torrents.iter().map(|t| t.id).collect();

    let arguments = json!({
        "ids": ids
    });

    request_transmission_with_retry(&client, "torrent-stop", arguments).await?;
    Ok(())
}

pub fn get_media_type_from_path(path: &str, tv_path: &str, movie_path: &str) -> Option<Media> {
    if path.starts_with(tv_path) {
        Some(Media::TV)
    } else if path.starts_with(movie_path) {
        Some(Media::Movie)
    } else {
        None
    }
}

pub fn get_storage_info() -> Result<String, String> {
    use sysinfo::{System, SystemExt, DiskExt};
    
    let mut system = System::new_all();
    system.refresh_disks_list();
    system.refresh_disks();

    let mut info = String::from("💾 Storage Information:\n\n");

    for disk in system.disks() {
        let total = disk.total_space();
        let available = disk.available_space();
        let used = total - available;
        let mount_point = disk.mount_point().to_string_lossy().to_string();
        
        let usage_percent = if total > 0 {
            (used as f64 / total as f64) * 100.0
        } else {
            0.0
        };

        info.push_str(&format!(
            "📁 {}:\n",
            mount_point
        ));
        info.push_str(&format!(
            "  Total: {}\n",
            format_bytes(total)
        ));
        info.push_str(&format!(
            "  Used: {} ({:.1}%)\n",
            format_bytes(used),
            usage_percent
        ));
        info.push_str(&format!(
            "  Available: {}\n\n",
            format_bytes(available)
        ));
    }

    Ok(info)
}

fn format_bytes(bytes: u64) -> String {
    use size_format::SizeFormatterSI;
    SizeFormatterSI::new(bytes).to_string()
}
