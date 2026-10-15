use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::collections::HashSet;
use tokio::process::Command;
use std::time::Duration;

use crate::transmission::Media;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuessitMetadata {
    pub title: String,
    pub year: Option<i32>,
    pub season: Option<u32>,
    pub episode: Option<serde_json::Value>,  // Can be single number or array
    #[serde(default)]
    pub extension: String,
}

impl GuessitMetadata {
    pub fn episodes(&self) -> Vec<u32> {
        match &self.episode {
            Some(serde_json::Value::Number(n)) => {
                if let Some(ep) = n.as_u64() {
                    vec![ep as u32]
                } else {
                    Vec::new()
                }
            }
            Some(serde_json::Value::Array(arr)) => {
                arr.iter()
                    .filter_map(|v| v.as_u64().map(|n| n as u32))
                    .collect()
            }
            _ => Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct MoveOperation {
    pub source_path: String,
    pub target_path: String,
    pub display_name: String,
    pub is_subtitle: bool,
}

#[derive(Debug, Clone)]
pub struct RestructurePlan {
    pub media_type: Media,
    pub operations: Vec<MoveOperation>,
    pub unparseable_files: Vec<String>,
}

const VIDEO_EXTENSIONS: &[&str] = &[
    ".mkv", ".mp4", ".avi", ".mov", ".wmv", ".flv", ".webm", ".m4v",
];

const SUBTITLE_EXTENSIONS: &[&str] = &[".srt", ".sub", ".ass", ".ssa", ".vtt"];

/// Recursively scan directory for video files
fn scan_files_recursive(dir: &str, extensions: &[&str]) -> Result<Vec<String>, String> {
    let mut files = Vec::new();
    let path = Path::new(dir);

    if !path.exists() {
        return Err(format!("Directory does not exist: {}", dir));
    }

    if !path.is_dir() {
        return Err(format!("Path is not a directory: {}", dir));
    }

    fn walk_dir(path: &Path, extensions: &[&str], files: &mut Vec<String>) -> Result<(), String> {
        let entries = std::fs::read_dir(path)
            .map_err(|e| format!("Failed to read directory {}: {}", path.display(), e))?;

        for entry in entries {
            let entry = entry.map_err(|e| format!("Failed to read entry: {}", e))?;
            let entry_path = entry.path();

            // Skip hidden files
            if let Some(file_name) = entry_path.file_name() {
                if let Some(name_str) = file_name.to_str() {
                    if name_str.starts_with('.') {
                        continue;
                    }
                }
            }

            if entry.file_type().map_err(|e| e.to_string())?.is_symlink() {
                continue;
            }
            if entry_path.is_dir() {
                walk_dir(&entry_path, extensions, files)?;
            } else if entry_path.is_file() {
                if let Some(ext) = entry_path.extension() {
                    if let Some(ext_str) = ext.to_str() {
                        let ext_with_dot = format!(".{}", ext_str.to_lowercase());
                        if extensions.contains(&ext_with_dot.as_str()) {
                            files.push(entry_path.to_string_lossy().to_string());
                        }
                    }
                }
            }
        }

        Ok(())
    }

    walk_dir(path, extensions, &mut files)?;
    files.sort();
    Ok(files)
}

/// Call guessit CLI to extract metadata
async fn call_guessit(file_path: &str, media: &Media) -> Result<GuessitMetadata, String> {
    let timeout = Duration::from_secs(5);

    let output = tokio::time::timeout(
        timeout,
        Command::new(guessit_executable())
            .kill_on_drop(true)
            .arg("-j")
            .arg("-t")
            .arg(match media { Media::TV => "episode", Media::Movie => "movie" })
            .arg("--")
            .arg(Path::new(file_path).file_name().ok_or("Missing filename")?)
            .output()
    )
    .await
    .map_err(|_| "guessit command timed out after 5 seconds".to_string())?
    .map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            "guessit not found. Install with: pip install guessit".to_string()
        } else {
            format!("Failed to execute guessit: {}", e)
        }
    })?;

    if !output.status.success() {
        return Err(format!(
            "guessit failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    let json_str = String::from_utf8_lossy(&output.stdout);
    let mut metadata: GuessitMetadata = serde_json::from_str(&json_str)
        .map_err(|e| format!("Failed to parse guessit output: {}", e))?;

    // Extract extension from file path
    metadata.extension = Path::new(file_path)
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|s| format!(".{}", s))
        .unwrap_or_else(|| ".mkv".to_string());

    Ok(metadata)
}

/// Sanitize filename by removing invalid characters
fn sanitize_filename(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '-',
            _ => c,
        })
        .collect()
}

/// Generate TV show path
fn generate_tv_path(base: &str, metadata: &GuessitMetadata) -> Result<String, String> {
    let season = metadata.season.ok_or("TV show missing season number")?;
    let episodes = metadata.episodes();

    if episodes.is_empty() {
        return Err("TV show missing episode number".to_string());
    }

    let title = safe_title(&metadata.title)?;
    let season_str = format!("{:02}", season);

    // Sort episodes and format as E01-E03 for multi-episode
    let mut sorted_episodes = episodes;
    sorted_episodes.sort();

    let episode_str = if sorted_episodes.len() == 1 {
        format!("E{:02}", sorted_episodes[0])
    } else {
        let ep_parts: Vec<String> = sorted_episodes
            .iter()
            .map(|e| format!("E{:02}", e))
            .collect();
        ep_parts.join("-")
    };

    let filename = format!("{} - S{}{}{}",
        title, season_str, episode_str, metadata.extension
    );

    let path = PathBuf::from(base)
        .join(&title)
        .join(format!("Season {}", season_str))
        .join(filename);

    Ok(path.to_string_lossy().to_string())
}

/// Generate movie path
fn generate_movie_path(base: &str, metadata: &GuessitMetadata) -> Result<String, String> {
    let title = safe_title(&metadata.title)?;

    let folder_name = if let Some(year) = metadata.year {
        format!("{} ({})", title, year)
    } else {
        title.clone()
    };

    let filename = format!("{}{}", folder_name, metadata.extension);

    let path = PathBuf::from(base)
        .join(&folder_name)
        .join(filename);

    Ok(path.to_string_lossy().to_string())
}

fn guessit_executable() -> PathBuf {
    std::env::var_os("GUESSIT_PATH").map(PathBuf::from).unwrap_or_else(|| {
        let local = PathBuf::from(".guessit-venv/bin/guessit");
        if local.is_file() { local } else { PathBuf::from("guessit") }
    })
}

// Existing curated folders are authoritative. Re-parsing them can strip words
// such as "Us" from show titles or mistake episode extras for separate shows.
fn already_curated(base: &str, file: &str, media: &Media) -> bool {
    let Ok(relative) = Path::new(file).strip_prefix(base) else { return false; };
    let parts: Vec<_> = relative.iter().filter_map(|p| p.to_str()).collect();
    if parts.iter().any(|p| matches!(*p, "Featurettes" | "Interviews" | "Behind The Scenes" | "Deleted Scenes" | "Trailers")) {
        return true;
    }
    match media {
        Media::TV => parts.len() == 3 && parts[1].strip_prefix("Season ")
            .map(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit())).unwrap_or(false),
        Media::Movie => parts.len() == 2 && parts[0].rsplit_once(" (")
            .map(|(_, year)| year.len() == 5 && year.ends_with(')')
                && year[..4].chars().all(|c| c.is_ascii_digit())).unwrap_or(false),
    }
}

fn safe_title(title: &str) -> Result<String, String> {
    let title = sanitize_filename(title).trim().trim_end_matches('.').to_string();
    if title.is_empty() || title == "." || title == ".." {
        return Err("Invalid or empty media title".into());
    }
    Ok(title)
}

// Only standardize folders. Keep the entire release filename for subtitles.
fn preserve_filename(target: &str, source: &str) -> Result<String, String> {
    let parent = Path::new(target).parent().ok_or("Missing target directory")?;
    let name = Path::new(source).file_name().ok_or("Missing source filename")?;
    Ok(parent.join(name).to_string_lossy().to_string())
}

/// Find matching subtitle files for a video file
fn find_matching_subtitles(video_path: &str) -> Vec<String> {
    let video = Path::new(video_path);
    let parent = match video.parent() {
        Some(p) => p,
        None => return Vec::new(),
    };

    let video_stem = match video.file_stem().and_then(|s| s.to_str()) {
        Some(s) => s,
        None => return Vec::new(),
    };

    let mut subtitles = Vec::new();

    if let Ok(entries) = std::fs::read_dir(parent) {
        for entry in entries.flatten() {
            let path = entry.path();

            if !path.is_file() {
                continue;
            }

            let Some(ext) = path.extension().and_then(|e| e.to_str()) else { continue; };
            {
                let ext_with_dot = format!(".{}", ext.to_lowercase());
                if !SUBTITLE_EXTENSIONS.contains(&ext_with_dot.as_str()) {
                    continue;
                }
            }

            if let Some(file_name) = path.file_name().and_then(|n| n.to_str()) {
                // Match exact name or name with language code
                // e.g., "show.s01e01.srt" or "show.s01e01.en.srt"
                if file_name.strip_prefix(video_stem).map(|rest| rest.starts_with('.')).unwrap_or(false) {
                    subtitles.push(path.to_string_lossy().to_string());
                }
            }
        }
    }

    subtitles.sort();
    subtitles
}

/// Generate complete restructure plan
pub async fn generate_restructure_plan(
    media: Media,
    base_path: &str,
) -> Result<RestructurePlan, String> {
    // Fail clearly once if the parser is unavailable, instead of silently skipping every file.
    let check = Command::new(guessit_executable()).arg("--version").kill_on_drop(true).output().await
        .map_err(|e| format!("GuessIt unavailable: {}", e))?;
    if !check.status.success() { return Err("GuessIt failed its startup check".into()); }
    // Scan for video files
    let video_files: Vec<_> = scan_files_recursive(base_path, VIDEO_EXTENSIONS)?
        .into_iter().filter(|file| !already_curated(base_path, file, &media)).collect();

    if video_files.is_empty() {
        return Ok(RestructurePlan {
            media_type: media,
            operations: Vec::new(),
            unparseable_files: Vec::new(),
        });
    }

    let mut operations = Vec::new();
    let mut reserved = HashSet::new();
    let mut unparseable_files = Vec::new();

    // Process files in batches of 10 concurrently
    let batch_size = 10;
    for chunk in video_files.chunks(batch_size) {
        let mut tasks = Vec::new();

        for file_path in chunk {
            let file_path = file_path.clone();
            let base_path = base_path.to_string();
            let media = media.clone();

            tasks.push(tokio::spawn(async move {
                (file_path.clone(), call_guessit(&file_path, &media).await, base_path, media)
            }));
        }

        // Wait for batch to complete
        for task in tasks {
            let (file_path, result, base_path, media) = task
                .await
                .map_err(|e| format!("Task failed: {}", e))?;

            match result {
                Ok(metadata) => {
                    // Generate target path
                    let target_path = match media {
                        Media::TV => generate_tv_path(&base_path, &metadata),
                        Media::Movie => generate_movie_path(&base_path, &metadata),
                    };

                    let target_path = match target_path {
                        Ok(p) => preserve_filename(&p, &file_path)?,
                        Err(_) => {
                            unparseable_files.push(file_path.clone());
                            continue;
                        }
                    };

                    // Skip if source == target (already organized)
                    let source_canonical = Path::new(&file_path)
                        .canonicalize()
                        .unwrap_or_else(|_| PathBuf::from(&file_path));
                    let target_canonical = Path::new(&target_path)
                        .canonicalize()
                        .unwrap_or_else(|_| PathBuf::from(&target_path));

                    if source_canonical == target_canonical {
                        continue;
                    }

                    // Never invent suffixes or overwrite another release.
                    if Path::new(&target_path).symlink_metadata().is_ok() || !reserved.insert(target_path.clone()) {
                        unparseable_files.push(file_path.clone());
                        continue;
                    }
                    let final_target = target_path;

                    // Get display name
                    let display_name = Path::new(&file_path)
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or(&file_path)
                        .to_string();

                    // Add video file operation
                    operations.push(MoveOperation {
                        source_path: file_path.clone(),
                        target_path: final_target.clone(),
                        display_name,
                        is_subtitle: false,
                    });

                    // Find and add subtitle operations
                    let subtitles = find_matching_subtitles(&file_path);
                    for sub_path in subtitles {
                        let sub_name = Path::new(&sub_path)
                            .file_name()
                            .and_then(|n| n.to_str())
                            .map(|s| s.to_string())
                            .unwrap_or_else(|| sub_path.clone());

                        // Generate subtitle target path (same directory as video)
                        let target_dir = Path::new(&final_target)
                            .parent()
                            .unwrap_or_else(|| Path::new(""));
                        let sub_target = target_dir.join(&sub_name);
                        let sub_target = sub_target.to_string_lossy().to_string();
                        if Path::new(&sub_target).symlink_metadata().is_ok() || !reserved.insert(sub_target.clone()) {
                            unparseable_files.push(sub_path);
                            continue;
                        }

                        operations.push(MoveOperation {
                            source_path: sub_path,
                            target_path: sub_target,
                            display_name: sub_name,
                            is_subtitle: true,
                        });
                    }
                }
                Err(_) => {
                    unparseable_files.push(file_path);
                }
            }
        }
    }

    Ok(RestructurePlan {
        media_type: media,
        operations,
        unparseable_files,
    })
}

/// Format the restructure plan for display
pub fn format_restructure_plan(plan: &RestructurePlan) -> String {
    if plan.operations.is_empty() && plan.unparseable_files.is_empty() {
        return "✅ Nothing to restructure".into();
    }
    let label = match plan.media_type { Media::TV => "TV", Media::Movie => "Movies" };
    let total = plan.operations.iter().filter(|op| !op.is_subtitle).count();
    let mut output = format!("{} restructure: {} videos, {} skipped\nOriginal filenames preserved; existing destinations never overwritten.\n\n", label, total, plan.unparseable_files.len());
    let mut shown = 0;
    for op in plan.operations.iter().filter(|op| !op.is_subtitle).take(10) {
        let row = format!("{}. {}\n → {}\n", shown + 1,
            crate::torrent_status::html_name(&op.display_name, 100),
            crate::torrent_status::html_name(&op.target_path, 180));
        if output.len() + row.len() > 3000 { break; }
        output.push_str(&row);
        shown += 1;
    }
    if shown < total { output.push_str(&format!("\nShowing {}/{} videos.\n", shown, total)); }
    if !plan.unparseable_files.is_empty() {
        output.push_str("\nSkipped (unparseable or conflicting):\n");
        for file in plan.unparseable_files.iter().take(3) {
            let name = Path::new(file).file_name().and_then(|n| n.to_str()).unwrap_or(file);
            output.push_str(&format!("• {}\n", crate::torrent_status::html_name(name, 100)));
        }
    }
    output.push_str("\nReply: apply all / apply 1 2 5 / cancel\nMoves do not update Transmission paths; moving seeded files may interrupt seeding.");
    output
}

/// Parse user's reply to select operations
pub fn parse_restructure_reply(
    reply_text: &str,
    plan: &RestructurePlan,
) -> Result<Vec<MoveOperation>, String> {
    let reply = reply_text.trim().to_lowercase();

    if reply == "cancel" {
        return Err("Restructure cancelled".to_string());
    }

    if reply == "apply all" || reply == "apply" || reply == "all" {
        return Ok(plan.operations.clone());
    }

    if reply.starts_with("apply ") {
        let indices_str = reply.strip_prefix("apply ").unwrap().trim();
        let mut indices: Vec<usize> = Vec::new();

        for part in indices_str.split_whitespace() {
            match part.parse::<usize>() {
                Ok(idx) => indices.push(idx),
                Err(_) => return Err(format!("Invalid number: {}", part)),
            }
        }

        // Remove duplicates
        indices.sort_unstable();
        indices.dedup();

        // Group operations by video file and collect selected ones
        let mut selected_ops = Vec::new();
        let mut current_index = 0;
        let mut i = 0;

        while i < plan.operations.len() {
            let op = &plan.operations[i];

            if !op.is_subtitle {
                current_index += 1;

                if indices.contains(&current_index) {
                    // Add the video file
                    selected_ops.push(op.clone());

                    // Add all associated subtitle files
                    let mut j = i + 1;
                    while j < plan.operations.len() && plan.operations[j].is_subtitle {
                        selected_ops.push(plan.operations[j].clone());
                        j += 1;
                    }
                    i = j;
                } else {
                    // Skip this video and its subtitles
                    let mut j = i + 1;
                    while j < plan.operations.len() && plan.operations[j].is_subtitle {
                        j += 1;
                    }
                    i = j;
                }
            } else {
                i += 1;
            }
        }

        if selected_ops.is_empty() {
            return Err("No valid operations selected".to_string());
        }

        // Validate all indices were in range
        let max_index = current_index;
        for idx in &indices {
            if *idx == 0 || *idx > max_index {
                return Err(format!("Index {} out of range (1-{})", idx, max_index));
            }
        }

        Ok(selected_ops)
    } else {
        Err("Invalid reply. Use 'apply all', 'apply 1 2 5', or 'cancel'".to_string())
    }
}

/// Execute the move operations
pub async fn execute_moves(operations: &[MoveOperation]) -> Result<String, String> {
    let mut success_count = 0;
    let mut errors = Vec::new();

    for op in operations {
        let source = Path::new(&op.source_path);
        let target = Path::new(&op.target_path);

        // Create target directory
        if let Some(parent) = target.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                errors.push(format!("{}: Failed to create directory - {}", op.display_name, e));
                continue;
            }
        }

        if source == target { continue; }
        match std::fs::symlink_metadata(source) {
            Ok(meta) if meta.is_file() && !meta.file_type().is_symlink() => {},
            _ => { errors.push(format!("{}: Source missing or not a regular file", op.display_name)); continue; }
        }
        match move_no_replace(source, target) {
            Ok(()) => success_count += 1,
            Err(e) => errors.push(format!("{}: Move refused (source retained): {}", crate::torrent_status::html_name(&op.display_name, 100), e)),
        }
    }

    let total = operations.len();
    let mut result = format!("✅ Restructuring complete!\n• {}/{} files moved", success_count, total);

    if !errors.is_empty() {
        result.push_str(&format!("\n• {} errors:\n", errors.len()));
        for (idx, error) in errors.iter().take(10).enumerate() {
            result.push_str(&format!("  - {}\n", error));
            if idx == 9 && errors.len() > 10 {
                result.push_str(&format!("  ... and {} more errors\n", errors.len() - 10));
                break;
            }
        }
    }

    if success_count == 0 {
        Err(result)
    } else {
        Ok(result)
    }
}

// Linux renameat2 provides atomic no-overwrite moves, including on exFAT.
#[cfg(target_os = "linux")]
fn move_no_replace(source: &Path, target: &Path) -> std::io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    use std::ffi::CString;
    extern "C" {
        fn renameat2(oldfd: i32, old: *const std::os::raw::c_char,
                     newfd: i32, new: *const std::os::raw::c_char, flags: u32) -> i32;
    }
    let source = CString::new(source.as_os_str().as_bytes())?;
    let target = CString::new(target.as_os_str().as_bytes())?;
    let result = unsafe { renameat2(-100, source.as_ptr(), -100, target.as_ptr(), 1) };
    if result == 0 { Ok(()) } else { Err(std::io::Error::last_os_error()) }
}
#[cfg(not(target_os = "linux"))]
fn move_no_replace(_: &Path, _: &Path) -> std::io::Result<()> {
    Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "Safe moves require Linux renameat2"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn curated_titles_and_bonus_features_are_not_reparsed() {
        assert!(already_curated("/tv", "/tv/When They See Us/Season 01/release.mkv", &Media::TV));
        assert!(already_curated("/tv", "/tv/Sean Combs - The Reckoning/Season 01/part-1.mkv", &Media::TV));
        assert!(already_curated("/tv", "/tv/True Detective/Season 01/Featurettes/S01E01 - Preview.mkv", &Media::TV));
        assert!(already_curated("/movies", "/movies/Dune Part Two (2024)/Dune (2024).mkv", &Media::Movie));
        assert!(!already_curated("/movies", "/movies/Dune (2024) [1080p]/release.mkv", &Media::Movie));
        assert!(!already_curated("/tv", "/tv/release-pack/episode.mkv", &Media::TV));
        assert!(!already_curated("/tv", "/tv/Show/Season nonsense/episode.mkv", &Media::TV));
    }
    #[tokio::test]
    async fn preserved_release_names_subtitles_and_idempotence() {
        let root = std::env::temp_dir().join(format!("restructure-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let release = "Love.Island.S01E02.1080p.WEB-DL-GROUP.mkv";
        std::fs::write(root.join(release), b"video").unwrap();
        std::fs::write(root.join("Love.Island.S01E02.1080p.WEB-DL-GROUP.en.srt"), b"subtitle").unwrap();
        std::fs::write(root.join("Love.Island.S01E02.1080p.WEB-DL-GROUP-other.srt"), b"unrelated").unwrap();
        let plan = generate_restructure_plan(Media::TV, root.to_str().unwrap()).await.unwrap();
        assert_eq!(plan.operations.len(), 2);
        assert!(plan.operations[0].target_path.ends_with(&format!("Love Island/Season 01/{}", release)));
        execute_moves(&plan.operations).await.unwrap();
        assert_eq!(std::fs::read(&plan.operations[0].target_path).unwrap(), b"video");
        assert_eq!(std::fs::read(&plan.operations[1].target_path).unwrap(), b"subtitle");
        assert!(generate_restructure_plan(Media::TV, root.to_str().unwrap()).await.unwrap().operations.is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn titles_are_safe_and_movie_names_preserved() {
        assert!(safe_title("..").is_err());
        let meta = GuessitMetadata { title: "Dune".into(), year: Some(2021), season: None, episode: None, extension: ".mkv".into() };
        let target = generate_movie_path("/movies", &meta).unwrap();
        assert_eq!(preserve_filename(&target, "/downloads/Dune.2021.1080p-GROUP.mkv").unwrap(), "/movies/Dune (2021)/Dune.2021.1080p-GROUP.mkv");
    }
    #[test]
    fn preview_is_html_safe_and_bounded() {
        let op = MoveOperation { source_path: "source".into(), target_path: "<&🦉>".repeat(1000), display_name: "<&🦉>".repeat(1000), is_subtitle: false };
        let plan = RestructurePlan {media_type: Media::TV, operations: vec![op; 100], unparseable_files: vec!["<&🦉>".repeat(1000); 100]};
        let text = format_restructure_plan(&plan);
        assert!(text.len() < 3800);
        assert!(!text.contains('<'));
        assert!(text.contains("apply all"));
        assert!(text.contains("&amp;"));
    }
    #[tokio::test]
    async fn stale_preview_never_overwrites_destination() {
        let root = std::env::temp_dir().join(format!("restructure-collision-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let source = root.join("source.mkv"); let target = root.join("target.mkv");
        std::fs::write(&source, b"source").unwrap(); std::fs::write(&target, b"existing").unwrap();
        let op = MoveOperation {source_path: source.to_string_lossy().into(), target_path: target.to_string_lossy().into(), display_name: "test".into(), is_subtitle: false};
        assert!(execute_moves(&[op]).await.is_err());
        assert_eq!(std::fs::read(source).unwrap(), b"source");
        assert_eq!(std::fs::read(target).unwrap(), b"existing");
        std::fs::remove_dir_all(root).unwrap();
    }
}
