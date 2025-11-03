use hyper::{body::to_bytes, client, Body, Uri};
use std::env;
use std::fs;

use size_format::SizeFormatterSI;
use std::str::FromStr;
use url::form_urlencoded;

use crate::transmission::Media;

#[derive(serde::Deserialize)]
struct Indexer {
    #[serde(rename(deserialize = "Name"))]
    #[allow(dead_code)]
    name: String,
}

#[derive(serde::Deserialize, Clone)]
struct Torrent {
    #[serde(rename(deserialize = "Seeders"))]
    seeders: i64,
    #[serde(rename(deserialize = "MagnetUri"))]
    magnet_uri: Option<String>,
    #[serde(rename(deserialize = "Title"))]
    title: String,
    #[serde(rename(deserialize = "Category"))]
    categories: Vec<i64>,
    #[serde(rename(deserialize = "Size"))]
    size: u64,
    #[serde(rename(deserialize = "Link"))]
    torrent_url: Option<String>,
}

#[derive(serde::Deserialize)]
struct JackettResponse {
    #[serde(rename(deserialize = "Indexers"))]
    indexers: Vec<Indexer>,
    #[serde(rename(deserialize = "Results"))]
    results: Vec<Torrent>,
}


#[derive(Clone)]
pub struct TorrentLocation {
    pub content: String,
    pub is_magnet: bool
}

#[derive(Clone)]
pub struct TelegramJackettResponse {
    torrents: Vec<Torrent>,
}

fn jackett_url() -> String {
    match env::var("JACKETT_URL") {
        Ok(host) => host,
        Err(_) => String::from("http://localhost:9117"),
    }
}

fn jackett_token() -> Result<String, String> {
    match env::var("JACKETT_TOKEN") {
        Ok(token) => Ok(token),
        Err(_) => match env::var("JACKETT_DATA_DIR") {
            Ok(data_dir) => {
                let file_name = data_dir + "/ServerConfig.json";

                let file_content = fs::read_to_string(file_name);

                match file_content {
                    Ok(file_content) => {
                        let v = serde_json::from_str(file_content.as_str());

                        let value: serde_json::Value = match v {
                            Ok(v) => v,
                            Err(err) => return Err(format!("{}", err)),
                        };

                        if value["APIKey"] == serde_json::Value::Null {
                            Err("Jackett file does not have key APIKey".to_string())
                        } else {
                            Ok(value["APIKey"].as_str().unwrap().to_string())
                        }
                    }
                    Err(err) => Err(format!("File error {}", err)),
                }
            }
            Err(_) => Err(
                "Set JACKETT_TOKEN or JACKETT_DATA_DIR if jackett is in the same host".to_string(),
            ),
        },
    }
}

pub async fn request_jackett(query_string: String) -> Result<TelegramJackettResponse, String> {
    let https = hyper_rustls::HttpsConnector::with_native_roots();
    let client: client::Client<_> = client::Client::builder().build(https);

    let token = jackett_token()?;

    let encoded_path: String = form_urlencoded::Serializer::new(String::new())
        .append_pair("apikey", token.as_str())
        .append_pair("Query", query_string.as_str())
        .finish();

    let url = [
        jackett_url(),
        String::from("/api/v2.0/indexers/all/results?"),
        encoded_path,
    ]
    .join("");

    let uri = Uri::from_str(&url);
    if let Err(err) = uri {
        return Err(format!("Url misconfigured {}", err));
    }

    let jackett_response = client.get(uri.unwrap()).await;
    if let Err(err) = jackett_response {
        return Err(format!("Jacket Response: {}", err));
    }

    let body: Body = jackett_response.unwrap().into_body();
    let body = to_bytes(body).await;

    if let Err(err) = body {
        return Err(format!("From Jackett to body: {}", err));
    }

    let new_body = body.unwrap();
    let str = String::from_utf8_lossy(&new_body);

    let v = serde_json::from_str(&str);
    if let Err(err) = v {
        return Err(format!("Not JSON {}", err.to_string()));
    }

    let mut formatted_body: JackettResponse = v.unwrap();
    if formatted_body.indexers.len() == 0 && formatted_body.results.len() == 0 {
        return Err("Empty indexers. Please add one in your jackett configuration".to_string());
    }

    formatted_body.results.sort_by_key(|d1| -d1.seeders);
    let torrents = formatted_body.results.into_iter().take(20).collect();

    let response = TelegramJackettResponse { torrents };

    if response.torrents.len() == 0 {
        return Err("No results were returned for your search".to_string());
    }

    Ok(response)
}

pub fn format_telegram_response(response: TelegramJackettResponse) -> String {
    let info = format_torrent(response);

    format!("<pre>{}</pre>", info)
}

fn format_torrent(response: TelegramJackettResponse) -> String {
    return response
        .torrents
        .iter()
        .enumerate()
        .fold(String::from(""), |text, (i, t)| {
            text + format!(
                "{}. {} - {}B - {}\n",
                i + 1,
                t.title,
                SizeFormatterSI::new(t.size),
                t.seeders
            )
            .as_str()
        });
}

fn is_movie(categories: Vec<i64>) -> bool {
    return categories.iter().any(|c| c >= &2000 && c < &3000);
}

fn is_tv_show(categories: Vec<i64>) -> bool {
    return categories.iter().any(|c| c >= &3000 && c < &4000);
}

fn is_debug_enabled() -> bool {
    env::var("DEBUG_TORRENT").is_ok()
}

pub async fn get_torrent_location_from_url(torrent_url: String) -> Result<TorrentLocation, String> {
    use hyper::header::{LOCATION, USER_AGENT};
    use hyper::{Body, Request};
    use url::Url;

    let https = hyper_rustls::HttpsConnector::with_native_roots();
    let client: client::Client<_> = client::Client::builder().build(https);
    let mut current = torrent_url.clone();
    let mut redirects = 0usize;

    loop {
        if redirects > 5 {
            return Err(format!("Too many redirects while fetching torrent: {}", current));
        }

        let req = Request::builder()
            .method("GET")
            .uri(&current)
            .header(USER_AGENT, "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)")
            .body(Body::empty())
            .unwrap();

        let resp = client.request(req).await.map_err(|e| e.to_string())?;
        let status = resp.status();

        if status.is_redirection() {
            let loc = resp.headers().get(LOCATION).and_then(|v| v.to_str().ok());
            if let Some(loc) = loc {
                let next = Url::parse(&current)
                    .ok()
                    .and_then(|base| base.join(loc).ok())
                    .map(|u| u.to_string())
                    .unwrap_or_else(|| loc.to_string());

                if next.starts_with("magnet:") {
                    // ✅ Treat redirect-to-magnet as a magnet torrent
                    return Ok(TorrentLocation { content: next, is_magnet: true });
                }

                current = next;
                redirects += 1;
                continue;
            } else {
                return Err("Redirect response without Location header".to_string());
            }
        }

        // 200 OK — expect a .torrent file body
        let content = hyper::body::to_bytes(resp.into_body())
            .await
            .map_err(|e| e.to_string())?;

        if content.is_empty() {
            return Err(format!("Torrent file download returned empty content. URL: {}", current));
        }

        // ✅ Return base64-encoded .torrent
        return Ok(TorrentLocation { content: base64::encode(content), is_magnet: false });
    }
}

pub async fn dispatch_from_reply(
    index: u16,
    reply_text: String,
    torrents: Vec<TelegramJackettResponse>,
) -> Result<(Option<Media>, TorrentLocation), String> {
    let real_index = index - 1;

    let jackett = torrents.clone().into_iter().find(|response| {
        let decoded_response = decode_html_entities(
            format_torrent(response.clone())
                .split_whitespace()
                .collect::<String>()
        );

        return decoded_response == reply_text.split_whitespace().collect::<String>();
    });

    match jackett {
        Some(jackett) => {
            let torrent = jackett.torrents.iter().nth(real_index.into());

            match torrent {
                Some(torrent) => {
                    let location: TorrentLocation;

                    if torrent.magnet_uri.is_some() {
                        let magnet = torrent.magnet_uri.clone().unwrap();
                        if is_debug_enabled() {
                            println!("[DEBUG] Using magnet URI (length: {})", magnet.len());
                        }
                        if magnet.is_empty() {
                            return Err("Torrent has empty magnet URI. Please select another".to_string());
                        }
                        location = TorrentLocation { content: magnet, is_magnet: true };

                    } else if torrent.torrent_url.is_some() {
                        let url = torrent.torrent_url.clone().unwrap();
                        if is_debug_enabled() {
                            println!("[DEBUG] Downloading torrent from URL: {}", url);
                        }
                        let result = get_torrent_location_from_url(url).await;

                        if let Ok(loc) = result {
                            if is_debug_enabled() {
                                println!(
                                    "[DEBUG] Resolved torrent: is_magnet={}, content_len={}",
                                    loc.is_magnet,
                                    loc.content.len()
                                );
                            }
                            location = loc;
                        } else {
                            return Err(result.err().unwrap());
                        }
                    } else {
                        return Err("Torrent without URI. Please select another".to_string());
                    }

                    if is_tv_show(torrent.clone().categories) {
                        return Ok((Some(Media::TV), location));
                    } else if is_movie(torrent.clone().categories) {
                        return Ok((Some(Media::Movie), location));
                    } else {
                        return Ok((None, location));
                    }
                }
                None => Err("No torrent for the given index".to_string()),
            }
        }
        None => Err("Couldn't find torrent in the list".to_string()),
    }
}

fn decode_html_entities(input: String) -> String {
    input
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&#039;", "'")
}
