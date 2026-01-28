use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tokio::process::Command;
use std::time::Duration;

use crate::transmission::Media;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuessitMetadata {
    pub title: String,
    pub year: Option<i32>,
    pub season: Option<u32>,
    pub episode: Option<serde_json::Value>,  // Can be single number or array
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

            if entry_path.is_dir() {
                walk_dir(&entry_path, extensions, files)?;
            } else if entry_path.is_file() {
                if let Some(ext) = entry_path.extension() {
                    if let Some(ext_str) = ext.to_str() {
                        let ext_with_dot = format!(".{}", ext_str);
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
async fn call_guessit(file_path: &str) -> Result<GuessitMetadata, String> {
    let timeout = Duration::from_secs(5);

    let output = tokio::time::timeout(
        timeout,
        Command::new("guessit")
            .arg("-j")
            .arg(file_path)
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

    let title = sanitize_filename(&metadata.title);
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
    let title = sanitize_filename(&metadata.title);

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

/// Resolve file collisions by appending -1, -2, etc.
fn resolve_collision(target_path: &str) -> String {
    let path = Path::new(target_path);

    if !path.exists() {
        return target_path.to_string();
    }

    let parent = path.parent().unwrap_or_else(|| Path::new(""));
    let file_stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let extension = path.extension().and_then(|s| s.to_str()).unwrap_or("");

    for i in 1..=100 {
        let new_name = if extension.is_empty() {
            format!("{}-{}", file_stem, i)
        } else {
            format!("{}-{}.{}", file_stem, i, extension)
        };

        let new_path = parent.join(new_name);
        if !new_path.exists() {
            return new_path.to_string_lossy().to_string();
        }
    }

    // Fallback if we hit max iterations
    target_path.to_string()
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

            if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                let ext_with_dot = format!(".{}", ext);
                if !SUBTITLE_EXTENSIONS.contains(&ext_with_dot.as_str()) {
                    continue;
                }
            }

            if let Some(file_name) = path.file_name().and_then(|n| n.to_str()) {
                // Match exact name or name with language code
                // e.g., "show.s01e01.srt" or "show.s01e01.en.srt"
                if file_name.starts_with(video_stem) {
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
    // Scan for video files
    let video_files = scan_files_recursive(base_path, VIDEO_EXTENSIONS)?;

    if video_files.is_empty() {
        return Ok(RestructurePlan {
            media_type: media,
            operations: Vec::new(),
            unparseable_files: Vec::new(),
        });
    }

    let mut operations = Vec::new();
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
                (file_path.clone(), call_guessit(&file_path).await, base_path, media)
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
                        Ok(p) => p,
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

                    // Resolve collisions
                    let final_target = resolve_collision(&target_path);

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
                        let sub_target = resolve_collision(&sub_target.to_string_lossy());

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
        return "✅ Nothing to restructure".to_string();
    }

    let emoji = match plan.media_type {
        Media::TV => "📺",
        Media::Movie => "🎬",
    };

    let mut output = format!("{} Restructure Plan:\n\n", emoji);

    // Group operations by video file (video + its subtitles)
    let mut current_index = 0;
    let mut i = 0;
    while i < plan.operations.len() {
        let op = &plan.operations[i];

        if !op.is_subtitle {
            current_index += 1;

            // Stop at 50 operations to avoid message size limits
            if current_index > 50 {
                output.push_str(&format!(
                    "\n... and {} more operations (showing first 50)\n",
                    plan.operations.len() - i
                ));
                break;
            }

            // Show video file
            let target_display = Path::new(&op.target_path)
                .strip_prefix(
                    Path::new(&op.target_path)
                        .ancestors()
                        .nth(2)
                        .unwrap_or_else(|| Path::new(""))
                )
                .unwrap_or_else(|_| Path::new(&op.target_path));

            output.push_str(&format!(
                "{}. {}\n   → {}\n",
                current_index,
                op.display_name,
                target_display.display()
            ));

            // Show subtitle files indented
            let mut j = i + 1;
            while j < plan.operations.len() && plan.operations[j].is_subtitle {
                let sub_op = &plan.operations[j];
                output.push_str(&format!("      + {}\n", sub_op.display_name));
                j += 1;
            }
            i = j;
        } else {
            i += 1;
        }
    }

    // Add unparseable files warning
    if !plan.unparseable_files.is_empty() {
        output.push_str("\n⚠️ Unparseable files (will be skipped):\n");
        for (idx, file) in plan.unparseable_files.iter().take(20).enumerate() {
            let display = Path::new(file)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(file);
            output.push_str(&format!("  • {}\n", display));

            if idx == 19 && plan.unparseable_files.len() > 20 {
                output.push_str(&format!("  ... and {} more\n", plan.unparseable_files.len() - 20));
                break;
            }
        }
    }

    output.push_str("\nReply with:\n");
    output.push_str("• \"apply all\" - Execute all operations\n");
    output.push_str("• \"apply 1 2 5\" - Execute specific operations\n");
    output.push_str("• \"cancel\" - Cancel restructure\n");

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

    if reply == "apply all" || reply == "apply" {
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

        // Try rename first (fast, same filesystem)
        match std::fs::rename(source, target) {
            Ok(_) => {
                success_count += 1;
            }
            Err(e) => {
                // If cross-filesystem error, try copy + delete
                if e.raw_os_error() == Some(18) || e.kind() == std::io::ErrorKind::Other {
                    match std::fs::copy(source, target) {
                        Ok(_) => {
                            if let Err(del_err) = std::fs::remove_file(source) {
                                errors.push(format!(
                                    "{}: Copied but failed to delete source - {}",
                                    op.display_name, del_err
                                ));
                            } else {
                                success_count += 1;
                            }
                        }
                        Err(copy_err) => {
                            errors.push(format!("{}: Failed to copy - {}", op.display_name, copy_err));
                        }
                    }
                } else {
                    errors.push(format!("{}: Failed to move - {}", op.display_name, e));
                }
            }
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
