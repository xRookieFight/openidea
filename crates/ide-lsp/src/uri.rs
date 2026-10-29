//! `file://` URI ⇄ path conversion (RFC 8089, VS Code-compatible encoding).

use percent_encoding::{AsciiSet, CONTROLS, percent_decode_str, utf8_percent_encode};
use std::path::{Path, PathBuf};
use std::str::FromStr;

/// Characters escaped in a path; `/` and `:` (drive letters) stay literal.
const PATH: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'%')
    .add(b'<')
    .add(b'>')
    .add(b'?')
    .add(b'[')
    .add(b'\\')
    .add(b']')
    .add(b'^')
    .add(b'`')
    .add(b'{')
    .add(b'|')
    .add(b'}');

pub fn path_to_uri_string(path: &Path) -> String {
    let s = path.to_string_lossy();
    let s = if cfg!(windows) { s.replace('\\', "/") } else { s.into_owned() };
    let mut out = String::from("file://");
    if !s.starts_with('/') {
        out.push('/');
    }
    out.extend(utf8_percent_encode(&s, PATH));
    out
}

pub fn path_to_uri(path: &Path) -> lsp_types::Uri {
    let s = path_to_uri_string(path);
    lsp_types::Uri::from_str(&s).unwrap_or_else(|_| {
        // Unreachable for encoded paths; keep a syntactically valid fallback.
        lsp_types::Uri::from_str("file:///").expect("static URI parses")
    })
}

/// Percent-encodes raw non-ASCII characters in every `file:` URI string of a
/// server message, so `lsp_types::Uri` accepts it. Existing `%xx` escapes and
/// other strings are left alone.
pub fn escape_file_uris(v: &mut serde_json::Value) {
    use serde_json::Value;
    match v {
        // ok so some servers (kotlin-lsp) send `file:///…/Masaüstü/…` as is, which is not a valid URI
        // a raw space never appears in a real URI, so that's message text and stays as is
        Value::String(s) if s.starts_with("file:") && !s.is_ascii() && !s.contains(char::is_whitespace) => {
            *s = utf8_percent_encode(s, CONTROLS).to_string();
        }
        Value::Array(a) => a.iter_mut().for_each(escape_file_uris),
        Value::Object(o) => o.values_mut().for_each(escape_file_uris),
        _ => {}
    }
}

/// Converts a `file:` URI to a path; `None` for other schemes.
pub fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://").or_else(|| uri.strip_prefix("FILE://"))?;
    // Skip an authority (`file://localhost/...`).
    let path = match rest.find('/') {
        Some(0) => rest,
        Some(i) if rest[..i].eq_ignore_ascii_case("localhost") => &rest[i..],
        _ => return None,
    };
    let path = path.split(['?', '#']).next().unwrap_or(path);
    let decoded = percent_decode_str(path).decode_utf8().ok()?;
    if cfg!(windows) {
        // `/C:/x` → `C:\x`
        let trimmed = decoded.strip_prefix('/').filter(|s| s.as_bytes().get(1) == Some(&b':')).unwrap_or(&decoded);
        return Some(PathBuf::from(trimmed.replace('/', "\\")));
    }
    Some(PathBuf::from(decoded.into_owned()))
}

/// The IDE identity of a document URI: its path for `file:` URIs, else the
/// URI itself (a virtual document such as `jdt://contents/…`) plus the URI.
pub fn uri_target(uri: &str) -> (PathBuf, Option<String>) {
    match uri_to_path(uri) {
        Some(p) => (p, None),
        None => (PathBuf::from(uri), Some(uri.to_owned())),
    }
}

pub use ide_core::text::is_virtual_uri;

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn round_trip() {
        for p in ["/tmp/a b/c#d%e.rs", "/home/ü/x[1].ts", "/plain/path.rs"] {
            let uri = path_to_uri(Path::new(p));
            assert!(uri.as_str().starts_with("file:///"), "{}", uri.as_str());
            assert_eq!(uri_to_path(uri.as_str()).unwrap(), PathBuf::from(p));
        }
        assert_eq!(path_to_uri_string(Path::new("/a b")), "file:///a%20b");
        assert_eq!(uri_to_path("file://localhost/x/y"), Some(PathBuf::from("/x/y")));
        assert_eq!(uri_to_path("file:///x/%C3%BC"), Some(PathBuf::from("/x/ü")));
        assert_eq!(uri_to_path("jdt://contents/x"), None);
    }

    #[test]
    fn raw_unicode_uris_from_servers_parse() {
        // kotlin-lsp sends `file:///home/u/Masaüstü/…` without escaping the ü
        let mut v = serde_json::json!({
            "uri": "file:///home/u/Masaüstü/a%20b.kt",
            "diagnostics": [{ "message": "file:///Masaüstü stays as text", "relatedInformation": [
                { "location": { "uri": "file:///home/u/Masaüstü/c.kt" } }
            ] }],
            "other": "jdt://contents/ü"
        });
        escape_file_uris(&mut v);
        assert_eq!(v["uri"], "file:///home/u/Masa%C3%BCst%C3%BC/a%20b.kt");
        assert_eq!(
            v["diagnostics"][0]["relatedInformation"][0]["location"]["uri"],
            "file:///home/u/Masa%C3%BCst%C3%BC/c.kt"
        );
        assert_eq!(v["diagnostics"][0]["message"], "file:///Masaüstü stays as text");
        assert_eq!(v["other"], "jdt://contents/ü");
        let uri = lsp_types::Uri::from_str(v["uri"].as_str().unwrap()).unwrap();
        #[cfg(unix)]
        assert_eq!(uri_to_path(uri.as_str()), Some(PathBuf::from("/home/u/Masaüstü/a b.kt")));
    }

    #[test]
    fn virtual_targets() {
        let jdt = "jdt://contents/rt.jar/java.lang/String.class?=p/%5C/x.jar%3Cjava.lang(String.class";
        assert_eq!(uri_target(jdt), (PathBuf::from(jdt), Some(jdt.to_owned())));
        #[cfg(unix)]
        assert_eq!(uri_target("file:///a/b.rs"), (PathBuf::from("/a/b.rs"), None));
        assert!(is_virtual_uri(jdt));
        assert!(is_virtual_uri("untitled:Untitled-1"));
        assert!(!is_virtual_uri("file:///a"));
        assert!(!is_virtual_uri("/a/b:c"));
        assert!(!is_virtual_uri("C:\\x\\y"));
    }
}
