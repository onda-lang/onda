use super::*;

pub(super) fn lsp_document_path(uri: &str) -> Result<Option<PathBuf>, String> {
    match uri.split_once(':') {
        Some(("file", _)) => {
            let path = file_uri_to_path(uri)?;
            // Browser LSP sessions use file URIs as stable identities for
            // in-memory overlays. They have no host filesystem to inspect.
            #[cfg(not(target_family = "wasm"))]
            onda_frontend::ensure_no_symlink_components(&path)
                .map_err(|error| format!("unsupported filesystem-backed Onda document: {error}"))?;
            Ok(Some(normalize_path(&path)))
        }
        Some(("untitled", _)) => Ok(None),
        Some((scheme, _)) => Err(format!("unsupported uri scheme '{scheme}'")),
        None => Err(format!("invalid uri '{uri}'")),
    }
}

pub(super) fn file_uri_to_path(uri: &str) -> Result<PathBuf, String> {
    let rest = uri
        .strip_prefix("file://")
        .ok_or_else(|| format!("unsupported uri '{uri}', expected file:// uri"))?;
    let decoded = percent_decode(rest)?;
    #[cfg(windows)]
    {
        if decoded.starts_with("//") {
            return Ok(PathBuf::from(decoded.replace('/', "\\")));
        }
        let decoded =
            if decoded.len() >= 3 && decoded.starts_with('/') && decoded.as_bytes()[2] == b':' {
                decoded[1..].to_owned()
            } else {
                decoded
            };
        return Ok(PathBuf::from(decoded.replace('/', "\\")));
    }
    #[cfg(not(windows))]
    {
        Ok(PathBuf::from(decoded))
    }
}

pub(super) fn path_to_file_uri(path: &Path) -> String {
    let normalized = normalize_path(path);
    let raw = normalized.to_string_lossy();
    #[cfg(windows)]
    let raw = raw.strip_prefix(r"\\?\").unwrap_or(&raw);
    #[cfg(windows)]
    let raw = if raw.starts_with('\\') {
        raw.replace('\\', "/")
    } else {
        format!("/{}", raw.replace('\\', "/"))
    };
    #[cfg(not(windows))]
    let raw = raw.to_string();

    format!("file://{}", percent_encode_path(&raw))
}

fn percent_decode(input: &str) -> Result<String, String> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len() {
                return Err(format!("invalid percent escape in '{input}'"));
            }
            let hex = std::str::from_utf8(&bytes[index + 1..index + 3])
                .map_err(|_| format!("invalid percent escape in '{input}'"))?;
            let value = u8::from_str_radix(hex, 16)
                .map_err(|_| format!("invalid percent escape in '{input}'"))?;
            out.push(value);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(out).map_err(|err| format!("invalid utf-8 in uri: {err}"))
}

fn percent_encode_path(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        let ch = byte as char;
        let keep = ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | '~' | '/' | ':');
        if keep {
            out.push(ch);
        } else {
            out.push('%');
            out.push(hex_digit(byte >> 4));
            out.push(hex_digit(byte & 0x0f));
        }
    }
    out
}

fn hex_digit(value: u8) -> char {
    match value {
        0..=9 => (b'0' + value) as char,
        10..=15 => (b'A' + (value - 10)) as char,
        _ => unreachable!("hex digit out of range"),
    }
}

pub(super) fn normalize_path(path: &Path) -> PathBuf {
    if let Ok(canonical) = fs::canonicalize(path) {
        return canonical;
    }
    if path.is_absolute() {
        return path.to_path_buf();
    }
    std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join(path)
}
