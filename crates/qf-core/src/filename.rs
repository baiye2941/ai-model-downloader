//! 文件名提取与 Content-Disposition 解析
//!
//! 整合了散布在 qf-app、qf-protocol(http/quic)中的重复实现,
//! 提供统一的文件名提取入口:
//! - `extract_filename_from_url` — 从 URL 路径提取文件名(含 percent-decode)
//! - `parse_content_disposition` — 解析 Content-Disposition 头
//! - `extract_filename` — 先尝试 Content-Disposition,再回退到 URL

/// 从 URL 路径段提取文件名
///
/// 解析 URL 路径的最后一段,并对百分号编码做 UTF-8 解码。
/// 无路径段或解析失败时返回 `"unknown"`。
pub fn extract_filename_from_url(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|u| {
            let segment = u.path().rsplit('/').next().unwrap_or("");
            if segment.is_empty() {
                None
            } else {
                percent_decode(segment)
            }
        })
        .unwrap_or_else(|| "unknown".to_string())
}

/// 提取文件名:优先 Content-Disposition,回退到 URL
///
/// 如果 `content_disposition` 为 `None` 或解析失败,
/// 则从 URL 路径提取。
pub fn extract_filename(url: &str, content_disposition: Option<&str>) -> String {
    content_disposition
        .and_then(parse_content_disposition)
        .unwrap_or_else(|| extract_filename_from_url(url))
}

/// 解析 Content-Disposition 头中的文件名
///
/// 支持两种格式:
/// - `filename*=UTF-8''percent_encoded_name` (RFC 5987)
/// - `filename="name"` / `filename=name`
///
/// `filename*` 优先于 `filename`。
pub fn parse_content_disposition(value: &str) -> Option<String> {
    if let Some(pos) = value.find("filename*=") {
        let rest = &value[pos + 10..];
        if let Some(encoded) = rest.split(';').next() {
            let mut parts = encoded.splitn(3, '\'');
            let _charset = parts.next(); // 编码名称(如 UTF-8),当前不使用
            let _encoding = parts.next(); // 编码方式(如 '', 当前不使用
            if let Some(encoded_name) = parts.next()
                && let Some(decoded) = percent_decode(encoded_name)
                && !decoded.is_empty()
            {
                return Some(decoded);
            }
        }
    }
    if let Some(pos) = value.find("filename=") {
        let rest = &value[pos + 9..];
        let name = rest.trim_start().split(';').next().unwrap_or(rest);
        let name = name.trim_matches('"').trim();
        if !name.is_empty() {
            return Some(name.to_string());
        }
    }
    None
}

fn percent_decode(input: &str) -> Option<String> {
    let mut output = Vec::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Some(byte) = parse_hex_pair(bytes[i + 1], bytes[i + 2]) {
                output.push(byte);
                i += 3;
            } else {
                output.push(bytes[i]);
                i += 1;
            }
        } else {
            output.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(output).ok()
}

fn parse_hex_pair(high: u8, low: u8) -> Option<u8> {
    let h = hex_digit(high)?;
    let l = hex_digit(low)?;
    Some(h * 16 + l)
}

fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_filename_from_url_basic() {
        assert_eq!(
            extract_filename_from_url("https://example.com/path/to/file.zip"),
            "file.zip"
        );
    }

    #[test]
    fn test_extract_filename_from_url_with_query() {
        assert_eq!(
            extract_filename_from_url("https://example.com/file.zip?v=2&token=abc"),
            "file.zip"
        );
    }

    #[test]
    fn test_extract_filename_from_url_root_path() {
        assert_eq!(extract_filename_from_url("https://example.com/"), "unknown");
    }

    #[test]
    fn test_extract_filename_from_url_no_path() {
        assert_eq!(extract_filename_from_url("https://example.com"), "unknown");
    }

    #[test]
    fn test_extract_filename_from_url_percent_space() {
        assert_eq!(
            extract_filename_from_url("https://example.com/my%20file.txt"),
            "my file.txt"
        );
    }

    #[test]
    fn test_extract_filename_from_url_invalid_hex_preserves_literal() {
        assert_eq!(
            extract_filename_from_url("https://example.com/file%GG.txt"),
            "file%GG.txt"
        );
    }

    #[test]
    fn test_extract_filename_from_url_chinese_percent_encoded() {
        assert_eq!(
            extract_filename_from_url("https://example.com/%E4%B8%AD%E6%96%87.txt"),
            "中文.txt"
        );
    }

    #[test]
    fn test_extract_filename_from_url_invalid_url() {
        assert_eq!(extract_filename_from_url("not a url"), "unknown");
    }

    #[test]
    fn test_extract_filename_from_url_empty() {
        assert_eq!(extract_filename_from_url(""), "unknown");
    }

    #[test]
    fn test_extract_filename_prefers_content_disposition() {
        assert_eq!(
            extract_filename(
                "https://example.com/path/file.zip",
                Some("attachment; filename=\"report.pdf\"")
            ),
            "report.pdf"
        );
    }

    #[test]
    fn test_extract_filename_falls_back_to_url() {
        assert_eq!(
            extract_filename("https://example.com/file.zip", None),
            "file.zip"
        );
    }

    #[test]
    fn test_extract_filename_falls_back_when_disposition_empty() {
        assert_eq!(
            extract_filename("https://example.com/file.zip", Some("inline")),
            "file.zip"
        );
    }

    #[test]
    fn test_parse_content_disposition_with_quotes() {
        assert_eq!(
            parse_content_disposition(r#"attachment; filename="file.zip""#),
            Some("file.zip".to_string())
        );
    }

    #[test]
    fn test_parse_content_disposition_without_quotes() {
        assert_eq!(
            parse_content_disposition("attachment; filename=file.zip"),
            Some("file.zip".to_string())
        );
    }

    #[test]
    fn test_parse_content_disposition_filename_star_utf8() {
        assert_eq!(
            parse_content_disposition("attachment; filename*=UTF-8''my%20file.zip"),
            Some("my file.zip".to_string())
        );
    }

    #[test]
    fn test_parse_content_disposition_filename_star_chinese() {
        assert_eq!(
            parse_content_disposition("attachment; filename*=UTF-8''%E4%B8%AD%E6%96%87.pdf"),
            Some("中文.pdf".to_string())
        );
    }

    #[test]
    fn test_parse_content_disposition_filename_star_priority() {
        assert_eq!(
            parse_content_disposition(
                "attachment; filename=fallback.txt; filename*=UTF-8''real%20name.txt"
            ),
            Some("real name.txt".to_string())
        );
    }

    #[test]
    fn test_parse_content_disposition_empty() {
        assert_eq!(parse_content_disposition(""), None);
    }

    #[test]
    fn test_parse_content_disposition_no_filename() {
        assert_eq!(parse_content_disposition("inline"), None);
    }

    #[test]
    fn test_parse_content_disposition_empty_filename() {
        assert_eq!(
            parse_content_disposition(r#"attachment; filename="""#),
            None
        );
    }

    #[test]
    fn test_parse_content_disposition_trailing_semicolon() {
        assert_eq!(
            parse_content_disposition("attachment; filename=test.zip;"),
            Some("test.zip".to_string())
        );
    }

    #[test]
    fn test_percent_decode_multi_byte_utf8() {
        assert_eq!(
            percent_decode("%E4%B8%AD%E6%96%87"),
            Some("中文".to_string())
        );
    }

    #[test]
    fn test_percent_decode_no_encoding() {
        assert_eq!(
            percent_decode("filename.zip"),
            Some("filename.zip".to_string())
        );
    }

    #[test]
    fn test_percent_decode_invalid_utf8_returns_none() {
        assert_eq!(percent_decode("%FF%FE"), None);
    }
}
