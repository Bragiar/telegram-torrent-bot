//! Bounded HTML status replies; no dependency or Telegram API change required.
use crate::transmission::Torrent;
use size_format::SizeFormatterSI;

const MAX_TORRENTS: usize = 10;
// Bound the encoded UTF-8 bytes, a stricter bound than Telegram's 4096
// characters (or UTF-16 units), even before HTML entity parsing.
const MESSAGE_BUDGET: usize = 3800;
const NAME_BUDGET: usize = 220;

/// Escape each complete Unicode scalar before budgeting, so neither UTF-8
/// sequences nor HTML entities can be cut. Reserve space for an ellipsis.
pub(crate) fn html_name(name: &str, budget: usize) -> String {
    let mut result = String::new();
    for ch in name.chars() {
        let mut buf = [0; 4];
        let escaped = match ch {
            '&' => "&amp;",
            '<' => "&lt;",
            '>' => "&gt;",
            c if c.is_control() => " ",
            c => c.encode_utf8(&mut buf),
        };
        if result.len() + escaped.len() + '…'.len_utf8() > budget {
            result.push('…');
            break;
        }
        result.push_str(escaped);
    }
    result
}

pub fn format_status(torrents: &[Torrent]) -> String {
    if torrents.is_empty() {
        return "📊 Torrents — showing 0 of 0
No torrents found.".to_string();
    }

    let mut recent: Vec<_> = torrents.iter().collect();
    // Transmission does not promise API ordering. ID is only a deterministic
    // tiebreaker when torrents have the same addedDate (second precision).
    recent.sort_unstable_by_key(|t| (std::cmp::Reverse(t.added_date), std::cmp::Reverse(t.id)));
    recent.truncate(MAX_TORRENTS);

    let header = format!(
        "📊 Torrents — showing {} of {} (newest added first)\n\n",
        recent.len(), torrents.len()
    );
    // Split the remaining byte budget evenly, retaining every selected row.
    let row_budget = (MESSAGE_BUDGET - header.len()) / recent.len();
    let mut result = header;
    for torrent in recent {
        let emoji = match torrent.status {
            0 => "⏸️",
            1 | 3 | 5 => "⏳",
            2 => "🔍",
            4 => "⬇️",
            6 => "⬆️",
            _ => "❓",
        };
        let percent = (torrent.percent_done.clamp(0.0, 1.0) * 100.0) as u64;
        let details = format!(
            " ({}%)\n  Size: {}, Downloaded: {}, Uploaded: {}\n",
            percent,
            SizeFormatterSI::new(torrent.total_size.max(0) as u64),
            SizeFormatterSI::new(torrent.downloaded_ever.max(0) as u64),
            SizeFormatterSI::new(torrent.uploaded_ever.max(0) as u64),
        );
        let name_budget = NAME_BUDGET.min(row_budget - emoji.len() - 1 - details.len());
        result.push_str(emoji);
        result.push(' ');
        result.push_str(&html_name(&torrent.name, name_budget));
        result.push_str(&details);
    }
    debug_assert!(result.len() <= MESSAGE_BUDGET);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn torrent(id: i64, added_date: i64, name: &str) -> Torrent {
        // Exercise the actual RPC field mapping, not just struct construction.
        serde_json::from_value(serde_json::json!({
            "id": id, "addedDate": added_date, "name": name,
            "status": 4, "percentDone": 0.5, "downloadDir": "/test",
            "totalSize": 1000, "downloadedEver": 500, "uploadedEver": 0,
            "seedRatioLimit": 2.0, "seedIdleLimit": 30
        })).unwrap()
    }

    #[test]
    fn newest_ten_by_added_date_not_id_or_api_order() {
        let torrents: Vec<_> = (0..15).map(|i| torrent(100 - i, i, &format!("item-{i:02}"))).collect();
        let output = format_status(&torrents);
        assert!(output.contains("showing 10 of 15 (newest added first)"));
        let names: Vec<_> = output.lines().filter(|line| line.starts_with("⬇️ "))
            .map(|line| line.split_whitespace().nth(1).unwrap()).collect();
        let expected: Vec<_> = (5..15).rev().map(|i| format!("item-{i:02}")).collect();
        assert_eq!(names, expected);
        assert_eq!(torrents[0].id, 100); // Does not reorder caller's deletion list.
    }

    #[test]
    fn equal_dates_have_deterministic_tiebreaker() {
        let output = format_status(&[torrent(1, 42, "first"), torrent(2, 42, "second")]);
        assert!(output.find("second").unwrap() < output.find("first (").unwrap());
        assert!(output.contains("showing 2 of 2"));
    }

    #[test]
    fn empty_set_has_explicit_count() {
        assert_eq!(format_status(&[]), "📊 Torrents — showing 0 of 0
No torrents found.");
    }

    #[test]
    fn html_is_escaped_and_entities_are_never_cut() {
        assert_eq!(html_name("<b>A&B</b> 😀", NAME_BUDGET), "&lt;b&gt;A&amp;B&lt;/b&gt; 😀");
        assert_eq!(html_name("&&&&", 13), "&amp;&amp;…");
        assert_eq!(html_name("😀😀😀", 11), "😀😀…");
        assert_eq!(html_name("a\nb\0c", NAME_BUDGET), "a b c");
    }

    #[test]
    fn huge_unicode_and_html_names_stay_below_all_message_limits() {
        for name in ["😀漢e\u{301}👩‍💻".repeat(10000), "<&>".repeat(10000), "a".repeat(100000)] {
            let torrents: Vec<_> = (0..30).map(|i| {
                let mut t = torrent(i, i, &name);
                t.total_size = i64::MAX;
                t.downloaded_ever = i64::MAX;
                t.uploaded_ever = i64::MAX;
                t.percent_done = f64::MAX;
                t.status = 0;
                t
            }).collect();
            let output = format_status(&torrents);
            assert!(output.len() <= MESSAGE_BUDGET);
            assert!(output.encode_utf16().count() < 4096);
            assert_eq!(output.matches("  Size:").count(), 10);
            assert_eq!(output.matches('…').count(), 10);
            assert!(!output.contains('<'));
            assert!(!output.contains('>'));
            assert!(output.contains("showing 10 of 30"));
        }
    }
}
