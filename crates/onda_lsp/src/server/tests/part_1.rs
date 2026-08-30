    use super::diagnostics::{diagnostic_message, diagnostic_to_lsp};
    use super::path_utils::file_uri_to_path;
    use super::{
        initialize_result, latest_full_text, lsp_document_path, path_to_file_uri,
        DependencyFingerprintCache, DiagnosticDelay, DiagnosticJobResult,
        DiagnosticScheduleRequest, LspCore, LspServer, LspSession, Position,
        TextDocumentContentChangeEvent,
    };
    use onda_frontend::{DiagCode, Diagnostic, SourceManifest};
    use onda_semantics as onda_daemon;
    use serde_json::json;
    use std::fs;
    use std::io::Cursor;
    use std::path::{Path, PathBuf};
    use std::sync::{mpsc, Mutex, MutexGuard};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn mk_temp_dir(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("onda_lsp_{prefix}_{nanos}"));
        fs::create_dir_all(&dir).expect("create temp test dir");
        dir
    }

    fn write_file(path: &Path, text: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent");
        }
        fs::write(path, text).expect("write test file");
    }

    fn stdlib_cache_env_lock() -> MutexGuard<'static, ()> {
        static LOCK: Mutex<()> = Mutex::new(());
        LOCK.lock().expect("stdlib cache env lock")
    }

    struct EnvVarGuard {
        name: &'static str,
        previous: Option<std::ffi::OsString>,
    }

    impl EnvVarGuard {
        fn set_path(name: &'static str, value: &Path) -> Self {
            let previous = std::env::var_os(name);
            std::env::set_var(name, value);
            Self { name, previous }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            if let Some(previous) = &self.previous {
                std::env::set_var(self.name, previous);
            } else {
                std::env::remove_var(self.name);
            }
        }
    }

    fn clear_readonly_recursive(path: &Path) {
        let Ok(metadata) = fs::metadata(path) else {
            return;
        };
        if metadata.is_dir() {
            if let Ok(entries) = fs::read_dir(path) {
                for entry in entries.flatten() {
                    clear_readonly_recursive(&entry.path());
                }
            }
        }
        let mut permissions = metadata.permissions();
        if permissions.readonly() {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                permissions.set_mode(permissions.mode() | 0o200);
            }
            #[cfg(not(unix))]
            permissions.set_readonly(false);
            fs::set_permissions(path, permissions).ok();
        }
    }

    fn decode_lsp_messages(bytes: Vec<u8>) -> Vec<serde_json::Value> {
        let mut cursor = Cursor::new(bytes);
        let mut messages = Vec::new();
        while let Some(message) = super::read_lsp_message(&mut cursor).expect("decode lsp message")
        {
            messages.push(message);
        }
        messages
    }

    fn position_after(source: &str, needle: &str) -> Position {
        let end = source
            .rfind(needle)
            .map(|idx| idx + needle.len())
            .expect("needle should exist in source");
        let before = &source[..end];
        let line = before.bytes().filter(|b| *b == b'\n').count() as u32;
        let character = before
            .rsplit_once('\n')
            .map(|(_, tail)| tail.encode_utf16().count())
            .unwrap_or_else(|| before.encode_utf16().count()) as u32;
        Position { line, character }
    }

    fn completion_labels_for(
        server: &mut LspServer,
        path: &Path,
        source: &str,
        needle: &str,
    ) -> Vec<String> {
        completion_items_for(server, path, source, needle)
            .iter()
            .filter_map(|item| item["label"].as_str().map(ToOwned::to_owned))
            .collect()
    }

    fn completion_items_for(
        server: &mut LspServer,
        path: &Path,
        source: &str,
        needle: &str,
    ) -> Vec<serde_json::Value> {
        completion_items_for_with_context(server, path, source, needle, None)
    }

    fn completion_items_for_with_context(
        server: &mut LspServer,
        path: &Path,
        source: &str,
        needle: &str,
        context: Option<serde_json::Value>,
    ) -> Vec<serde_json::Value> {
        let normalized =
            server
                .session
                .open_document(path, onda_daemon::DocumentVersion(1), source.to_owned());
        let uri = path_to_file_uri(&normalized);
        let position = position_after(source, needle);
        let mut params = json!({
            "textDocument": { "uri": uri },
            "position": {
                "line": position.line,
                "character": position.character,
            }
        });
        if let Some(context) = context {
            params["context"] = context;
        }
        let mut writer = Vec::new();
        server
            .handle_message(
                json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "textDocument/completion",
                    "params": params
                }),
                &mut writer,
            )
            .expect("completion should succeed");

        let messages = decode_lsp_messages(writer);
        messages
            .iter()
            .find(|message| message["id"] == json!(1))
            .and_then(|message| message["result"]["items"].as_array())
            .expect("completion response items")
            .clone()
    }

    fn request_with_position(
        server: &mut LspServer,
        path: &Path,
        source: &str,
        needle: &str,
        method: &str,
    ) -> serde_json::Value {
        let normalized =
            server
                .session
                .open_document(path, onda_daemon::DocumentVersion(1), source.to_owned());
        let uri = path_to_file_uri(&normalized);
        let position = position_after(source, needle);
        let mut writer = Vec::new();
        server
            .handle_message(
                json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": method,
                    "params": {
                        "textDocument": { "uri": uri },
                        "position": {
                            "line": position.line,
                            "character": position.character,
                        }
                    }
                }),
                &mut writer,
            )
            .expect("lsp request should succeed");

        decode_lsp_messages(writer)
            .into_iter()
            .find(|message| message["id"] == json!(1))
            .and_then(|message| message.get("result").cloned())
            .expect("lsp result")
    }

    fn hover_markdown_for(
        server: &mut LspServer,
        path: &Path,
        source: &str,
        needle: &str,
    ) -> Option<String> {
        request_with_position(server, path, source, needle, "textDocument/hover")
            .pointer("/contents/value")
            .and_then(|value| value.as_str())
            .map(ToOwned::to_owned)
    }

    fn definition_for(
        server: &mut LspServer,
        path: &Path,
        source: &str,
        needle: &str,
    ) -> serde_json::Value {
        request_with_position(server, path, source, needle, "textDocument/definition")
    }

    fn document_symbols_for(
        server: &mut LspServer,
        path: &Path,
        source: &str,
    ) -> Vec<serde_json::Value> {
        let normalized =
            server
                .session
                .open_document(path, onda_daemon::DocumentVersion(1), source.to_owned());
        let uri = path_to_file_uri(&normalized);
        let mut writer = Vec::new();
        server
            .handle_message(
                json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "textDocument/documentSymbol",
                    "params": {
                        "textDocument": { "uri": uri }
                    }
                }),
                &mut writer,
            )
            .expect("document symbol request should succeed");

        decode_lsp_messages(writer)
            .into_iter()
            .find(|message| message["id"] == json!(1))
            .and_then(|message| message["result"].as_array().cloned())
            .expect("document symbol response")
    }

    fn semantic_token_data_for(server: &mut LspServer, path: &Path) -> Vec<u32> {
        let uri = path_to_file_uri(path);
        server
            .semantic_tokens_for_uri(&uri)
            .expect("semantic tokens should succeed")["data"]
            .as_array()
            .expect("semantic token data")
            .iter()
            .map(|value| value.as_u64().expect("semantic token integer") as u32)
            .collect()
    }

    #[test]
    fn latest_full_text_prefers_last_full_document_change() {
        let changes = vec![
            TextDocumentContentChangeEvent {
                range: Some(json!({ "start": 0 })),
                text: "partial".to_owned(),
            },
            TextDocumentContentChangeEvent {
                range: None,
                text: "full".to_owned(),
            },
        ];
        assert_eq!(latest_full_text(&changes), Some("full".to_owned()));
    }

    #[test]
    fn parsed_document_cache_reparses_after_document_change() {
        let dir = mk_temp_dir("parse_cache_reparse");
        let main = dir.join("main.onda");
        let old_source = r#"
namespace Old:
  const X = 1
"#;
        let new_source = r#"
namespace New:
  const X = 1
"#;
        write_file(&main, old_source);

        let mut server = LspServer::default();
        let old_symbols = document_symbols_for(&mut server, &main, old_source);
        let new_symbols = document_symbols_for(&mut server, &main, new_source);
        let old_names = old_symbols
            .iter()
            .filter_map(|symbol| symbol["name"].as_str())
            .collect::<Vec<_>>();
        let new_names = new_symbols
            .iter()
            .filter_map(|symbol| symbol["name"].as_str())
            .collect::<Vec<_>>();

        assert!(old_names.contains(&"Old"), "old symbols: {old_symbols:?}");
        assert!(new_names.contains(&"New"), "new symbols: {new_symbols:?}");
        assert!(
            !new_names.contains(&"Old"),
            "new symbols should not come from stale parse cache: {new_symbols:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn parsed_open_document_cache_refreshes_importing_document_after_diagnostics() {
        let dir = mk_temp_dir("parse_cache_open_importing_reparse");
        let lib = dir.join("lib.onda");
        let main = dir.join("main.onda");
        let old_source = r#"
include "lib.onda"

namespace Old:
  const X = 1
"#;
        let new_source = r#"
include "lib.onda"

namespace New:
  const X = 1
"#;
        write_file(&lib, "namespace Imported:\n  const X = 1\n");
        write_file(&main, old_source);

        let mut server = LspServer::default();
        let old_symbols = document_symbols_for(&mut server, &main, old_source);
        let normalized = server.session.update_document(
            &main,
            onda_daemon::DocumentVersion(2),
            new_source.to_owned(),
        );
        server
            .publish_diagnostics_for_entry(&normalized, &mut Vec::new())
            .expect("diagnostics should refresh parsed snapshot");
        let new_symbols = document_symbols_for(&mut server, &main, new_source);
        let old_names = old_symbols
            .iter()
            .filter_map(|symbol| symbol["name"].as_str())
            .collect::<Vec<_>>();
        let new_names = new_symbols
            .iter()
            .filter_map(|symbol| symbol["name"].as_str())
            .collect::<Vec<_>>();

        assert!(old_names.contains(&"Old"), "old symbols: {old_symbols:?}");
        assert!(new_names.contains(&"New"), "new symbols: {new_symbols:?}");
        assert!(
            !new_names.contains(&"Old"),
            "changed importing source should refresh after diagnostics: {new_symbols:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_reparses_after_valid_document_change() {
        let dir = mk_temp_dir("completion_cache_reparse");
        let main = dir.join("main.onda");
        let old_source = r#"
namespace Old:
  const X = 1

sample:
  out1 = O
"#;
        let new_source = r#"
namespace New:
  const X = 1

sample:
  out1 = N
"#;
        write_file(&main, old_source);

        let mut server = LspServer::default();
        let old_labels = completion_labels_for(&mut server, &main, old_source, "O");
        let new_labels = completion_labels_for(&mut server, &main, new_source, "N");

        assert!(
            old_labels.contains(&"Old".to_owned()),
            "old completion should see old AST: {old_labels:?}"
        );
        assert!(
            new_labels.contains(&"New".to_owned()),
            "completion should reparse valid changed text: {new_labels:?}"
        );
        assert!(
            !new_labels.contains(&"Old".to_owned()),
            "completion should not use stale parse when changed text parses: {new_labels:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn unresolved_dependency_creation_invalidates_the_parse_cache() {
        let dir = mk_temp_dir("parse_cache_unresolved_dependency");
        let lib = dir.join("lib.onda");
        let main = dir.join("main.onda");
        let source = "import lib\nsample:\n  out1 = target()\n";
        write_file(&main, source);

        let mut server = LspServer::default();
        let overlays = server.session.overlay_map();
        let missing_fingerprint = server.document_fingerprint_for_path(&main, source, &overlays);
        assert!(
            server
                .parse_cache
                .get(&super::normalize_path(&main))
                .is_some_and(|cached| cached.parsed.is_none()),
            "the unresolved import should initially fail to parse"
        );
        assert!(
            server
                .dependency_fingerprint_cache
                .source_depends_on_path(&main, source, &lib, &overlays, None,),
            "an unresolved candidate must still count as a dependency"
        );

        write_file(&lib, "def target():\n  return 0.0\n");
        let resolved_fingerprint = server.document_fingerprint_for_path(&main, source, &overlays);

        assert_ne!(
            resolved_fingerprint, missing_fingerprint,
            "creating an unresolved dependency must invalidate the cached fingerprint"
        );
        assert!(
            server
                .parse_cache
                .get(&super::normalize_path(&main))
                .is_some_and(|cached| cached.parsed.is_some()),
            "the importer should be reparsed after its dependency appears"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn parsed_document_cache_refreshes_after_dependency_diagnostics() {
        let dir = mk_temp_dir("parse_cache_dependency_reparse");
        let lib = dir.join("lib.onda");
        let main = dir.join("main.onda");
        let source = r#"
import lib

outs:
  out1

sample:
  out1 = Lib::target()
"#;
        write_file(
            &lib,
            r#"
namespace Lib:
  def target():
    return 0.0
"#,
        );
        write_file(&main, source);

        let mut server = LspServer::default();
        let old_definition = definition_for(&mut server, &main, source, "Lib::target");

        write_file(
            &lib,
            r#"
namespace Lib:
  const Spacer = 1

  def target():
    return 0.0
	"#,
        );
        server
            .publish_diagnostics_for_entry(&main, &mut Vec::new())
            .expect("diagnostics should refresh imported definition snapshot");
        let new_definition = definition_for(&mut server, &main, source, "Lib::target");

        assert_ne!(
            old_definition["range"]["start"]["line"],
            new_definition["range"]["start"]["line"],
            "definition should reflect changed imported file after diagnostics, old={old_definition:?}, new={new_definition:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn definition_for_open_importing_document_parses_before_diagnostics() {
        let dir = mk_temp_dir("definition_open_importing_before_diagnostics");
        let main = dir.join("main.onda");
        let source = r#"
import std/delay

proc CyberCell<T>:
  ins<T>:
    src
    fb

  outs<T> 1

  params<T>:
    drive = 1.0
    bias = 0.0

  sample 32:
    x = (src + fb + bias) * drive
    out1 = x

init:
  smear = std::delay::Delay<f64>()
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        for (needle, expected_line) in [("src", 5), ("fb", 6), ("bias", 12), ("drive", 11)] {
            let definition = definition_for(&mut server, &main, source, needle);
            assert_ne!(
                definition,
                json!(null),
                "{needle} should resolve before diagnostics populate the parse cache"
            );
            assert_eq!(
                definition["range"]["start"]["line"],
                json!(expected_line),
                "{needle} should resolve to its proc-local declaration: {definition:?}"
            );
        }

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn definition_for_open_importing_document_resolves_stdlib_proc_before_diagnostics() {
        let dir = mk_temp_dir("definition_open_importing_stdlib_before_diagnostics");
        let cache = dir.join("cache");
        let _env_lock = stdlib_cache_env_lock();
        let _guard = EnvVarGuard::set_path("ONDA_STDLIB_CACHE_DIR", &cache);
        let main = dir.join("main.onda");
        let source = r#"
import std/delay

init:
  smear = std::delay::Delay<f64>()
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let definition = definition_for(&mut server, &main, source, "Delay");

        assert_ne!(
            definition,
            json!(null),
            "stdlib proc should resolve before diagnostics populate the parse cache"
        );
        let uri = definition["uri"].as_str().expect("stdlib proc uri");
        let path = file_uri_to_path(uri).expect("stdlib proc should be a file uri");
        assert!(
            path.starts_with(&cache),
            "stdlib proc should materialize inside cache: {}",
            path.display()
        );
        assert_eq!(
            path.file_name().and_then(|name| name.to_str()),
            Some("delay.onda")
        );
        let line = definition["range"]["start"]["line"]
            .as_u64()
            .expect("proc line") as usize;
        let materialized = fs::read_to_string(&path).expect("read materialized stdlib");
        let target_line = materialized
            .lines()
            .nth(line)
            .expect("definition line should exist");
        assert!(
            target_line.contains("proc Delay"),
            "definition should target std::delay::Delay, got line: {target_line}"
        );

        drop(_guard);
        clear_readonly_recursive(&dir);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn parsed_document_cache_tracks_on_import_dependencies_after_diagnostics() {
        let dir = mk_temp_dir("parse_cache_on_dependency_reparse");
        let lib = dir.join("lib.on");
        let main = dir.join("main.onda");
        let source = r#"
import lib

outs:
  out1

sample:
  out1 = Lib::target()
"#;
        write_file(
            &lib,
            r#"
namespace Lib:
  def target():
    return 0.0
"#,
        );
        write_file(&main, source);

        let mut server = LspServer::default();
        let old_definition = definition_for(&mut server, &main, source, "Lib::target");

        write_file(
            &lib,
            r#"
namespace Lib:
  const Spacer = 1

  def target():
    return 0.0
	"#,
        );
        server
            .publish_diagnostics_for_entry(&main, &mut Vec::new())
            .expect("diagnostics should refresh .on imported definition snapshot");
        let new_definition = definition_for(&mut server, &main, source, "Lib::target");

        assert_ne!(
            old_definition["range"]["start"]["line"],
            new_definition["range"]["start"]["line"],
            "definition should reflect changed .on imported file after diagnostics, old={old_definition:?}, new={new_definition:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn semantic_token_cache_reparses_after_dependency_change() {
        let dir = mk_temp_dir("semantic_token_cache_dependency_reparse");
        let lib = dir.join("lib.onda");
        let main = dir.join("main.onda");
        let source = r#"
import lib

sample:
  out1 = target()
"#;
        write_file(
            &lib,
            r#"
def target():
  return 0.0
"#,
        );
        write_file(&main, source);

        let mut server = LspServer::default();
        let old_data = semantic_token_data_for(&mut server, &main);

        write_file(
            &lib,
            r#"
const target = 0.0
"#,
        );
        let new_data = semantic_token_data_for(&mut server, &main);

        assert_ne!(
            old_data, new_data,
            "semantic token cache should account for imported file changes"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn diagnostics_refresh_requests_semantic_tokens_after_parsed_importer_update() {
        let dir = mk_temp_dir("semantic_token_cache_diagnostic_refresh");
        let lib = dir.join("lib.onda");
        let main = dir.join("main.onda");
        let main_source = r#"
import lib
use sc

sample:
  out1 = 0.0
"#;
        let old_lib_source = r#"
def sc():
  return 0.0
"#;
        let new_lib_source = r#"
namespace sc:
  namespace SinOsc:
    const A = 1
"#;
        write_file(&lib, old_lib_source);
        write_file(&main, main_source);

        let mut server = LspServer {
            semantic_tokens_refresh: true,
            ..LspServer::default()
        };
        let main_path =
            server
                .session
                .open_document(&main, onda_daemon::DocumentVersion(1), main_source);
        let lib_path =
            server
                .session
                .open_document(&lib, onda_daemon::DocumentVersion(1), old_lib_source);

        server
            .publish_diagnostics_for_entry(&main_path, &mut Vec::new())
            .expect("initial diagnostics should publish");
        let old_data = semantic_token_data_for(&mut server, &main_path);
        assert!(
            !old_data.is_empty(),
            "initial semantic token data should be non-empty"
        );

        server
            .session
            .update_document(&lib_path, onda_daemon::DocumentVersion(2), new_lib_source);
        server.note_document_changed(&lib_path);
        let mut writer = Vec::new();
        server
            .publish_diagnostics_for_entry(&main_path, &mut writer)
            .expect("refreshed diagnostics should publish");
        let messages = decode_lsp_messages(writer);

        assert!(
            messages.iter().any(|message| {
                message["method"]
                    .as_str()
                    .map(|method| method == "workspace/semanticTokens/refresh")
                    .unwrap_or(false)
            }),
            "parsed diagnostic refresh should request semantic-token refresh: {messages:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn semantic_tokens_for_open_document_do_not_scan_import_dependencies() {
        let dir = mk_temp_dir("semantic_tokens_open_no_dependency_scan");
        let lib = dir.join("lib.onda");
        let main = dir.join("main.onda");
        let source = r#"
import lib

outs:
  out1

sample:
  out1 = target()
"#;
        write_file(
            &lib,
            r#"
def target():
  return 0.0
"#,
        );
        write_file(&main, source);

        let mut server = LspServer::default();
        let normalized =
            server
                .session
                .open_document(&main, onda_daemon::DocumentVersion(1), source);
        let data = semantic_token_data_for(&mut server, &normalized);

        assert!(
            !data.is_empty(),
            "open document should produce semantic tokens"
        );
        assert!(
            server.dependency_fingerprint_cache.disk_files.is_empty(),
            "open-document semantic tokens should not walk imported files"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn dependency_edit_refreshes_open_importer_semantic_tokens_immediately() {
        let dir = mk_temp_dir("semantic_tokens_open_dependency_edit");
        let lib = dir.join("lib.onda");
        let main = dir.join("main.onda");
        let main_source = "import lib\nsample:\n  out1 = target()\n";
        let old_lib = "def target():\n  return 0.0\n";
        let new_lib = "const target = 0.0\n";
        write_file(&lib, old_lib);
        write_file(&main, main_source);

        let mut server = LspServer::default();
        let main_path =
            server
                .session
                .open_document(&main, onda_daemon::DocumentVersion(1), main_source);
        let lib_path = server
            .session
            .open_document(&lib, onda_daemon::DocumentVersion(1), old_lib);
        let old_data = semantic_token_data_for(&mut server, &main_path);

        let changed =
            server
                .session
                .update_document(&lib_path, onda_daemon::DocumentVersion(2), new_lib);
        server.note_document_changed(&changed);
        let new_data = semantic_token_data_for(&mut server, &main_path);

        assert_ne!(
            old_data, new_data,
            "semantic tokens should use the edited overlay"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_uses_cached_parse_after_unsaved_edit() {
        let dir = mk_temp_dir("completion_cached_parse_after_edit");
        let main = dir.join("main.onda");
        let source = r#"
namespace sc:
  namespace SinOsc:
    proc ar:
      outs:
        out1
      sample:
        out1 = 0.0

use sc

init:
  b = Sin

sample:
  out1 = 0.0
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let normalized =
            server
                .session
                .open_document(&main, onda_daemon::DocumentVersion(1), source);
        let fingerprint = super::source_fingerprint(source);
        let parsed = onda_frontend::parse_program_file_with_overlays(
            &normalized,
            &server.session.overlay_map(),
        )
        .expect("source should parse");
        server.cache_parsed_program_for_path(&normalized, fingerprint, Some(parsed));
        server.note_document_changed(&normalized);

        let labels = completion_labels_for(&mut server, &main, source, "Sin");

        assert!(
            labels.contains(&"SinOsc".to_owned()),
            "completion should reuse cached namespace index after edit: {labels:?}"
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_for_open_document_reuses_cached_imported_symbols_without_dependency_scan() {
        let dir = mk_temp_dir("completion_open_reparse_no_dependency_scan");
        let lib = dir.join("lib.onda");
        let main = dir.join("main.onda");
        let initial_source = r#"
include "lib.onda"

use sc

init:
  a = SinOsc::ar()

sample:
  out1 = a()
"#;
        let edited_source = r#"
include "lib.onda"

use sc

init:
  a = SinOsc::ar()
  d = S

sample:
  out1 = a()
"#;
        write_file(
            &lib,
            r#"
namespace sc:
  namespace SinOsc:
    proc ar:
      outs:
        out1
      sample:
        out1 = 0.0
"#,
        );
        write_file(&main, initial_source);

        let mut server = LspServer::default();
        let normalized =
            server
                .session
                .open_document(&main, onda_daemon::DocumentVersion(1), initial_source);
        let parsed = onda_frontend::parse_program_file_with_overlays(
            &normalized,
            &server.session.overlay_map(),
        )
        .expect("initial source should parse");
        server.cache_parsed_program_for_path(
            &normalized,
            super::source_fingerprint(initial_source),
            Some(parsed),
        );
        server
            .session
            .update_document(&main, onda_daemon::DocumentVersion(2), edited_source);
        server.dependency_fingerprint_cache.clear();

        let labels = completion_labels_for(&mut server, &main, edited_source, "d = S");

        assert!(
            labels.contains(&"SinOsc".to_owned()),
            "completion should keep imported namespace symbols from the cached parse: {labels:?}"
        );
        assert!(
            server.dependency_fingerprint_cache.disk_files.is_empty(),
            "open-document completion should not walk imported files"
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn diagnostics_do_not_retain_stale_parse_after_syntax_error() {
        let dir = mk_temp_dir("diagnostic_error_drops_stale_parse_cache");
        let lib = dir.join("lib.onda");
        let main = dir.join("main.onda");
        let initial_source = r#"
include "lib.onda"

use sc

init:
  t = 0.0

sample:
  out1 = 0.0
"#;
        let invalid_source = r#"
include "lib.onda"

use sc

init:
  t =

sample:
  out1 = 0.0
"#;
        let completion_source = r#"
include "lib.onda"

use sc

init:
  t = Si

sample:
  out1 = 0.0
"#;
        write_file(
            &lib,
            r#"
namespace sc:
  namespace SinOsc:
    const A = 1
"#,
        );
        write_file(&main, initial_source);

        let mut server = LspServer::default();
        let normalized =
            server
                .session
                .open_document(&main, onda_daemon::DocumentVersion(1), initial_source);
        server
            .publish_diagnostics_for_entry(&normalized, &mut Vec::new())
            .expect("initial diagnostics should cache parsed snapshot");
        assert!(
            server
                .parse_cache
                .get(&normalized)
                .and_then(|cached| cached.parsed.as_ref())
                .is_some(),
            "initial successful diagnostics should cache a parsed snapshot"
        );

        server
            .session
            .update_document(&main, onda_daemon::DocumentVersion(2), invalid_source);
        server.note_document_changed(&main);
        server
            .publish_diagnostics_for_entry(&normalized, &mut Vec::new())
            .expect("syntax diagnostics should publish");
        assert!(
            server
                .parse_cache
                .get(&normalized)
                .and_then(|cached| cached.parsed.as_ref())
                .is_none(),
            "failed diagnostics should not keep a stale parsed snapshot"
        );

        server
            .session
            .update_document(&main, onda_daemon::DocumentVersion(3), completion_source);
        server.note_document_changed(&main);
        let labels = completion_labels_for(&mut server, &main, completion_source, "Si");
        assert!(
            labels.contains(&"SinOsc".to_owned()),
            "completion should reparse current source after an error: {labels:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_rebuilds_index_when_parsed_dependencies_change() {
        let dir = mk_temp_dir("completion_rebuilds_changed_dependency_index");
        let lib = dir.join("lib.onda");
        let main = dir.join("main.onda");
        let source = r#"
include "lib.onda"

use sc

init:
  d = S

sample:
  out1 = 0.0
"#;
        write_file(
            &lib,
            r#"
namespace sc:
  namespace SinOsc:
    const A = 1
"#,
        );
        write_file(&main, source);

        let mut server = LspServer::default();
        let old_parsed = onda_frontend::parse_program_file_with_overlays(
            &super::normalize_path(&main),
            &server.session.overlay_map(),
        )
        .expect("initial source should parse");
        server.cache_parsed_program_for_path(
            &main,
            super::source_fingerprint(source),
            Some(old_parsed),
        );
        let labels = completion_labels_for(&mut server, &main, source, "d = S");
        assert!(
            labels.contains(&"SinOsc".to_owned()),
            "initial completion should build ready index: {labels:?}"
        );

        write_file(
            &lib,
            r#"
namespace sc:
  namespace SawOsc:
    const A = 1
"#,
        );
        let parsed = onda_frontend::parse_program_file_with_overlays(
            &super::normalize_path(&main),
            &server.session.overlay_map(),
        )
        .expect("changed dependency should parse");
        server.cache_parsed_program_for_path(
            &main,
            super::source_fingerprint(source),
            Some(parsed),
        );

        let labels = completion_labels_for(&mut server, &main, source, "d = S");
        assert!(
            labels.contains(&"SawOsc".to_owned()),
            "completion should use the replacement parsed dependency: {labels:?}"
        );
        assert!(
            !labels.contains(&"SinOsc".to_owned()),
            "completion should not retain stale dependency symbols: {labels:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn namespace_completion_for_open_document_reuses_cached_imported_symbols_without_dependency_scan(
    ) {
        let dir = mk_temp_dir("completion_open_namespace_reparse_no_dependency_scan");
        let lib = dir.join("lib.onda");
        let main = dir.join("main.onda");
        let initial_source = r#"
include "lib.onda"

use sc

init:
  a = SinOsc::ar()

sample:
  out1 = a()
"#;
        let edited_source = r#"
include "lib.onda"

use sc

init:
  a = SinOsc::ar()
  d = SinOsc::

sample:
  out1 = a()
"#;
        write_file(
            &lib,
            r#"
namespace sc:
  namespace SinOsc:
    proc ar:
      outs:
        out1
      sample:
        out1 = 0.0
"#,
        );
        write_file(&main, initial_source);

        let mut server = LspServer::default();
        let normalized =
            server
                .session
                .open_document(&main, onda_daemon::DocumentVersion(1), initial_source);
        let parsed = onda_frontend::parse_program_file_with_overlays(
            &normalized,
            &server.session.overlay_map(),
        )
        .expect("initial source should parse");
        server.cache_parsed_program_for_path(
            &normalized,
            super::source_fingerprint(initial_source),
            Some(parsed),
        );
        server
            .session
            .update_document(&main, onda_daemon::DocumentVersion(2), edited_source);
        server.dependency_fingerprint_cache.clear();

        let labels = completion_labels_for(&mut server, &main, edited_source, "SinOsc::");

        assert!(
            labels.contains(&"ar".to_owned()),
            "namespace completion should keep imported namespace members from the cached parse: {labels:?}"
        );
        assert!(
            server.dependency_fingerprint_cache.disk_files.is_empty(),
            "open-document namespace completion should not walk imported files"
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn diagnostic_entries_include_open_importers_after_dependency_change() {
        let dir = mk_temp_dir("diagnostic_dependency_importers");
        let lib = dir.join("lib.onda");
        let main = dir.join("main.onda");
        let main_source = "import lib\nouts:\n  out1\nsample:\n  out1 = Lib::target()\n";
        write_file(&lib, "namespace Lib:\n  def target():\n    return 0.0\n");
        write_file(&main, main_source);

        let mut server = LspServer::default();
        let main_path =
            server
                .session
                .open_document(&main, onda_daemon::DocumentVersion(1), main_source);
        let lib_path = server.session.open_document(
            &lib,
            onda_daemon::DocumentVersion(2),
            "namespace Lib:\n  def target():\n    return invalid\n",
        );

        let affected = server.diagnostic_entries_affected_by_change(&lib_path);
        assert!(
            affected.contains(&main_path),
            "importing entry should be re-diagnosed after dependency change: {affected:?}"
        );
        assert!(
            affected.contains(&lib_path),
            "changed open document should be diagnosed: {affected:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn dependency_change_keeps_unrelated_entry_caches() {
        let dir = mk_temp_dir("targeted_dependency_invalidation");
        let first_lib = dir.join("first_lib.onda");
        let second_lib = dir.join("second_lib.onda");
        let first_main = dir.join("first_main.onda");
        let second_main = dir.join("second_main.onda");
        let first_lib_source = "const first = 1.0\n";
        let second_lib_source = "const second = 2.0\n";
        let first_main_source = "import first_lib\nsample:\n  out1 = first\n";
        let second_main_source = "import second_lib\nsample:\n  out1 = second\n";
        write_file(&first_lib, first_lib_source);
        write_file(&second_lib, second_lib_source);
        write_file(&first_main, first_main_source);
        write_file(&second_main, second_main_source);

        let mut server = LspServer::default();
        let first_main = server.session.open_document(
            &first_main,
            onda_daemon::DocumentVersion(1),
            first_main_source,
        );
        let second_main = server.session.open_document(
            &second_main,
            onda_daemon::DocumentVersion(1),
            second_main_source,
        );
        let first_lib = server.session.open_document(
            &first_lib,
            onda_daemon::DocumentVersion(1),
            first_lib_source,
        );
        server
            .publish_diagnostics_for_entry(&first_main, &mut Vec::new())
            .expect("cache first entry");
        server
            .publish_diagnostics_for_entry(&second_main, &mut Vec::new())
            .expect("cache second entry");
        assert!(server.parse_cache.contains_key(&first_main));
        assert!(server.parse_cache.contains_key(&second_main));

        server.session.update_document(
            &first_lib,
            onda_daemon::DocumentVersion(2),
            "const first = 3.0\n",
        );
        let affected = server.note_document_changed(&first_lib);

        assert!(affected.contains(&first_main));
        assert!(affected.contains(&first_lib));
        assert!(!affected.contains(&second_main));
        assert!(!server.parse_cache.contains_key(&first_main));
        assert!(
            server.parse_cache.contains_key(&second_main),
            "an unrelated source graph should retain its parse cache"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn dependency_edit_invalidates_importer_completion_immediately() {
        let dir = mk_temp_dir("dependency_edit_completion_invalidation");
        let lib = dir.join("lib.onda");
        let main = dir.join("main.onda");
        let main_source = "import lib\ninit:\n  value = Lib::Old\n";
        let old_lib = "namespace Lib:\n  const Old = 1\n";
        let new_lib = "namespace Lib:\n  const New = 1\n";
        write_file(&lib, old_lib);
        write_file(&main, main_source);

        let mut server = LspServer::default();
        server
            .session
            .open_document(&main, onda_daemon::DocumentVersion(1), main_source);
        server
            .session
            .open_document(&lib, onda_daemon::DocumentVersion(1), old_lib);
        let old_labels = completion_labels_for(&mut server, &main, main_source, "Lib::");
        assert!(
            old_labels.contains(&"Old".to_owned()),
            "labels: {old_labels:?}"
        );

        let changed =
            server
                .session
                .update_document(&lib, onda_daemon::DocumentVersion(2), new_lib);
        server.note_document_changed(&changed);
        let new_labels = completion_labels_for(&mut server, &main, main_source, "Lib::");

        assert!(
            new_labels.contains(&"New".to_owned()),
            "labels: {new_labels:?}"
        );
        assert!(
            !new_labels.contains(&"Old".to_owned()),
            "stale imported symbol should be gone: {new_labels:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn dependency_edit_refreshes_importer_navigation_immediately() {
        let dir = mk_temp_dir("dependency_edit_navigation_invalidation");
        let lib = dir.join("lib.onda");
        let main = dir.join("main.onda");
        let main_source = "import lib\nsample:\n  out1 = target()\n";
        let old_lib = "def target():\n  return 0.0\n";
        let new_lib = "\n\ndef target():\n  return 1.0\n";
        write_file(&lib, old_lib);
        write_file(&main, main_source);

        let mut server = LspServer::default();
        server
            .session
            .open_document(&main, onda_daemon::DocumentVersion(1), main_source);
        let lib_path = server
            .session
            .open_document(&lib, onda_daemon::DocumentVersion(1), old_lib);
        let old_definition = definition_for(&mut server, &main, main_source, "target");

        let changed =
            server
                .session
                .update_document(&lib_path, onda_daemon::DocumentVersion(2), new_lib);
        server.note_document_changed(&changed);
        let new_definition = definition_for(&mut server, &main, main_source, "target");

        assert_ne!(
            old_definition["range"]["start"]["line"], new_definition["range"]["start"]["line"],
            "navigation should use the edited dependency overlay"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_triggers_include_call_argument_contexts() {
        let result = initialize_result(None);
        let triggers = result["capabilities"]["completionProvider"]["triggerCharacters"]
            .as_array()
            .expect("completion trigger characters")
            .iter()
            .filter_map(|value| value.as_str())
            .collect::<Vec<_>>();

        assert!(triggers.contains(&"("), "triggers: {triggers:?}");
        assert!(triggers.contains(&","), "triggers: {triggers:?}");
        assert!(triggers.contains(&"{"), "triggers: {triggers:?}");
    }

    #[test]
    fn initialize_advertises_signature_help_for_calls() {
        let result = initialize_result(None);
        assert_eq!(
            result["capabilities"]["signatureHelpProvider"]["triggerCharacters"],
            json!(["(", ","])
        );
    }

    #[test]
    fn completion_offers_unused_parameter_domain_fields() {
        let dir = mk_temp_dir("completion_param_domain_fields");
        let main = dir.join("main.onda");
        let source = "params:\n  cutoff = 440.0 {min = 20, ";
        write_file(&main, source);

        let mut server = LspServer {
            completion_snippets: true,
            ..LspServer::default()
        };
        let items = completion_items_for(&mut server, &main, source, source);
        let labels = items
            .iter()
            .filter_map(|item| item["label"].as_str())
            .collect::<Vec<_>>();

        assert!(!labels.contains(&"min"), "items: {items:?}");
        for expected in ["max", "scale", "curve", "unit", "step"] {
            assert!(labels.contains(&expected), "missing {expected}: {items:?}");
        }
        let scale = items
            .iter()
            .find(|item| item["label"] == json!("scale"))
            .expect("scale field completion");
        assert_eq!(scale["kind"], json!(10));
        assert_eq!(scale["detail"], json!("parameter domain field"));
        assert_eq!(scale["insertText"], json!("scale = ${1|linear,log|}"));
        assert_eq!(scale["insertTextFormat"], json!(2));
        let curve = items
            .iter()
            .find(|item| item["label"] == json!("curve"))
            .expect("curve field completion");
        assert_eq!(curve["insertText"], json!("curve = $1"));

        let shorthand_source = "params:\n  cutoff = 440.0 {20000, scale = linear, ";
        write_file(&main, shorthand_source);
        let items = completion_items_for(&mut server, &main, shorthand_source, shorthand_source);
        let labels = items
            .iter()
            .filter_map(|item| item["label"].as_str())
            .collect::<Vec<_>>();
        assert!(labels.contains(&"min"), "items: {items:?}");
        assert!(labels.contains(&"max"), "items: {items:?}");

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_only_offers_count_for_buffer_annotations() {
        let dir = mk_temp_dir("completion_buffer_count_field");
        let main = dir.join("main.onda");
        let source = "buffers:\n  bank: f32 {";
        write_file(&main, source);

        let mut server = LspServer {
            completion_snippets: true,
            ..LspServer::default()
        };
        let items = completion_items_for(&mut server, &main, source, source);
        assert_eq!(items.len(), 1, "items: {items:?}");
        assert_eq!(items[0]["label"], json!("count"));
        assert_eq!(items[0]["kind"], json!(10));
        assert_eq!(items[0]["detail"], json!("buffer count field"));
        assert_eq!(items[0]["insertText"], json!("count = $1"));
        assert_eq!(items[0]["insertTextFormat"], json!(2));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_treats_log_as_a_contextual_parameter_scale() {
        let dir = mk_temp_dir("completion_param_domain_log");
        let main = dir.join("main.onda");
        let named_source = "params:\n  cutoff = 440.0 {min = 20, max = 20000, scale = lo";
        write_file(&main, named_source);

        let mut server = LspServer::default();
        let named_items = completion_items_for(&mut server, &main, named_source, "scale = lo");
        assert_eq!(named_items.len(), 1, "items: {named_items:?}");
        assert_eq!(named_items[0]["label"], json!("log"));
        assert_eq!(named_items[0]["kind"], json!(20));
        assert_eq!(named_items[0]["detail"], json!("parameter scale"));

        let positional_source = "params:\n  cutoff = 440.0 {20, 20000, lo";
        let positional_items =
            completion_items_for(&mut server, &main, positional_source, positional_source);
        assert_eq!(positional_items.len(), 1, "items: {positional_items:?}");
        assert_eq!(positional_items[0]["label"], json!("log"));
        assert_eq!(positional_items[0]["kind"], json!(20));

        let expression_source = "params:\n  cutoff = 440.0 {lo";
        let expression_items =
            completion_items_for(&mut server, &main, expression_source, expression_source);
        let expression_log = expression_items
            .iter()
            .find(|item| item["label"] == json!("log"))
            .expect("stdlib log expression completion");
        assert_eq!(expression_log["kind"], json!(3));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_colon_trigger_ignores_single_colon() {
        let dir = mk_temp_dir("completion_single_colon_trigger");
        let main = dir.join("main.onda");
        let source = "namespace Foo:\n  const A = 1\n";
        write_file(&main, source);

        let mut server = LspServer::default();
        let items = completion_items_for_with_context(
            &mut server,
            &main,
            source,
            "Foo:",
            Some(json!({
                "triggerKind": 2,
                "triggerCharacter": ":",
            })),
        );

        assert!(
            items.is_empty(),
            "single colon trigger should not produce completions: {items:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_colon_trigger_allows_double_colon() {
        let dir = mk_temp_dir("completion_double_colon_trigger");
        let main = dir.join("main.onda");
        let source = "namespace Foo:\n  const A = 1\n\nsample:\n  out1 = Foo::\n";
        write_file(&main, source);

        let mut server = LspServer::default();
        let labels = completion_items_for_with_context(
            &mut server,
            &main,
            source,
            "Foo::",
            Some(json!({
                "triggerKind": 2,
                "triggerCharacter": ":",
            })),
        )
        .iter()
        .filter_map(|item| item["label"].as_str().map(ToOwned::to_owned))
        .collect::<Vec<_>>();

        assert!(
            labels.contains(&"A".to_owned()),
            "double colon trigger should produce namespace completions: {labels:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_expands_count_shorthand_proc_call_args() {
        let dir = mk_temp_dir("completion_count_shorthand_proc_args");
        let main = dir.join("main.onda");
        let source = r#"
proc Counted:
  ins 2
  params 2
  outs 1

  sample:
    out1 = in1

init:
  counted = Counted()

sample:
  out1 = counted(
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let labels = completion_labels_for(&mut server, &main, source, "counted(");

        for expected in ["in1", "in2", "param1", "param2"] {
            assert!(
                labels.contains(&expected.to_owned()),
                "expected {expected} in labels: {labels:?}"
            );
        }

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_expands_imported_std_count_shorthand_proc_call_args() {
        let dir = mk_temp_dir("completion_std_count_shorthand_proc_args");
        let main = dir.join("main.onda");
        let source = r#"
import std/convolution

init:
  conv = std::convolution<64, 1024>::ZeroLatencyConvolver<f32>()

sample:
  out1 = conv(
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let labels = completion_labels_for(&mut server, &main, source, "conv(");

        assert!(
            labels.contains(&"in1".to_owned()),
            "expected std convolver input in labels: {labels:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_resolves_use_symbol_alias_for_proc_instance_call_args() {
        let dir = mk_temp_dir("completion_use_symbol_alias_proc_call_args");
        let main = dir.join("main.onda");
        let source = r#"
namespace Fx:
  proc Convolver:
    ins:
      input
    outs:
      out1
    params:
      gain = 1.0

    sample:
      out1 = input * gain

use Fx::Convolver as Conv

outs:
  out1

init:
  conv = Conv()

sample:
  out1 = conv(
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let labels = completion_labels_for(&mut server, &main, source, "conv(");

        assert!(
            labels.contains(&"input".to_owned()),
            "expected aliased proc input in labels: {labels:?}"
        );
        assert!(
            labels.contains(&"gain".to_owned()),
            "expected aliased proc param in labels: {labels:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_resolves_use_symbol_alias_for_function_call_args() {
        let dir = mk_temp_dir("completion_use_symbol_alias_function_args");
        let main = dir.join("main.onda");
        let source = r#"
namespace Fx:
  def mix(input, gain):
    return input * gain

use Fx::mix as mx

outs:
  out1

sample:
  out1 = mx(
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let labels = completion_labels_for(&mut server, &main, source, "mx(");

        assert!(
            labels.contains(&"input".to_owned()),
            "expected aliased def arg in labels: {labels:?}"
        );
        assert!(
            labels.contains(&"gain".to_owned()),
            "expected aliased def arg in labels: {labels:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_resolves_use_namespace_alias_for_constructor_call_args() {
        let dir = mk_temp_dir("completion_use_namespace_alias_constructor_args");
        let main = dir.join("main.onda");
        let source = r#"
namespace Fx:
  proc Convolver:
    params:
      gain = 1.0

    sample:
      out1 = gain

use Fx as fx

init:
  conv = fx::Convolver(
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let labels = completion_labels_for(&mut server, &main, source, "fx::Convolver(");

        assert!(
            labels.contains(&"gain".to_owned()),
            "expected aliased constructor param in labels: {labels:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_switches_to_expression_items_after_named_arg_equals() {
        let dir = mk_temp_dir("completion_named_arg_value");
        let main = dir.join("main.onda");
        let source = concat!(
            "proc Counted:\n",
            "  ins 1\n",
            "  outs 1\n",
            "\n",
            "  sample:\n",
            "    out1 = in1\n",
            "\n",
            "params:\n",
            "  gain = 1.0\n",
            "\n",
            "init:\n",
            "  counted = Counted()\n",
            "\n",
            "sample:\n",
            "  out1 = counted(in1 = "
        );
        write_file(&main, source);

        let mut server = LspServer::default();
        let labels = completion_labels_for(&mut server, &main, source, "in1 = ");

        assert!(
            labels.contains(&"gain".to_owned()),
            "named arg value should complete expression symbols: {labels:?}"
        );
        assert!(
            !labels.contains(&"in1".to_owned()),
            "named arg value should not repeat the named arg itself: {labels:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_returns_to_named_args_after_named_arg_value_comma() {
        let dir = mk_temp_dir("completion_named_arg_after_comma");
        let main = dir.join("main.onda");
        let source = concat!(
            "proc Counted:\n",
            "  ins 2\n",
            "  outs 1\n",
            "\n",
            "  sample:\n",
            "    out1 = in1\n",
            "\n",
            "params:\n",
            "  gain = 1.0\n",
            "\n",
            "init:\n",
            "  counted = Counted()\n",
            "\n",
            "sample:\n",
            "  out1 = counted(in1 = gain, "
        );
        write_file(&main, source);

        let mut server = LspServer::default();
        let labels = completion_labels_for(&mut server, &main, source, "gain, ");

        assert!(
            labels.contains(&"in1".to_owned()),
            "after a comma, completion should offer named args again: {labels:?}"
        );
        assert!(
            labels.contains(&"in2".to_owned()),
            "after a comma, completion should offer remaining proc inputs: {labels:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn diagnostic_message_appends_trace() {
        let diagnostic = Diagnostic {
            code: DiagCode::Semantic,
            message: "root error".to_owned(),
            line: 1,
            column: 1,
            end_line: 1,
            end_column: 2,
            file: None,
            trace: vec!["deep".to_owned(), "higher".to_owned()],
            editor_visible: true,
        };
        let message = diagnostic_message(&diagnostic);
        assert!(message.contains("root error"));
        assert!(message.contains("trace:"));
        assert!(message.contains("- higher"));
        assert!(message.contains("- deep"));
    }

    #[test]
    fn diagnostic_to_lsp_uses_descriptive_code() {
        let diagnostic = Diagnostic {
            code: DiagCode::Syntax,
            message: "expected graph arrow".to_owned(),
            line: 1,
            column: 1,
            end_line: 1,
            end_column: 2,
            file: None,
            trace: Vec::new(),
            editor_visible: true,
        };

        let lsp = diagnostic_to_lsp(&diagnostic);
        assert_eq!(lsp["code"], json!("syntax"));
    }

    #[test]
    fn file_uri_round_trips_common_paths() {
        let path = if cfg!(windows) {
            std::path::PathBuf::from(r"C:\Users\franc\Sources\onda llvm\file.onda")
        } else {
            std::path::PathBuf::from("/tmp/onda llvm/file.onda")
        };
        let uri = path_to_file_uri(&path);
        let decoded = file_uri_to_path(&uri).expect("file uri should decode");
        assert_eq!(decoded, path);
    }

    #[test]
    fn untitled_uri_is_accepted_without_disk_path() {
        assert_eq!(
            lsp_document_path("untitled:Scratch-1").expect("untitled uri should be accepted"),
            None
        );
    }

    #[cfg(unix)]
    #[test]
    fn lsp_document_paths_reject_symlinks() {
        use std::os::unix::fs::symlink;

        let dir = mk_temp_dir("symlink_document");
        let target = dir.join("main.onda");
        let alias = dir.join("linked.onda");
        write_file(&target, "sample:\n  out1 = 0.0\n");
        symlink(&target, &alias).expect("create document symlink");

        let error = lsp_document_path(&format!("file://{}", alias.display()))
            .expect_err("LSP documents must reject symlinks");
        assert!(error.contains("symlink component"));

        let cached = dir.join("cached.onda");
        write_file(&cached, "const value = 1.0\n");
        let mut cache = DependencyFingerprintCache::default();
        cache
            .disk_file_summary(&cached)
            .expect("cache regular dependency");
        fs::remove_file(&cached).expect("remove regular dependency");
        symlink(&target, &cached).expect("replace dependency with symlink");
        assert_eq!(
            cache
                .disk_file_summary(&cached)
                .expect_err("cached dependencies must reject symlink replacements"),
            std::io::ErrorKind::InvalidInput
        );

        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn publish_diagnostics_does_not_immediately_clear_entry_uri() {
        let dir = mk_temp_dir("publish_diagnostics");
        let main = dir.join("main.onda");
        write_file(&main, "sample:\n  out1 = missing\n");

        let mut server = LspServer::default();
        let normalized = server.session.open_document(
            &main,
            onda_daemon::DocumentVersion(1),
            fs::read_to_string(&main).expect("read test file"),
        );
        let uri = path_to_file_uri(&normalized);
        server.document_uris.insert(normalized, uri.clone());

        let mut writer = Vec::new();
        server
            .publish_diagnostics_for_entry(&main, &mut writer)
            .expect("publish diagnostics");

        let notifications = decode_lsp_messages(writer)
            .into_iter()
            .filter(|message| {
                message["method"]
                    .as_str()
                    .map(|method| method == "textDocument/publishDiagnostics")
                    .unwrap_or(false)
            })
            .collect::<Vec<_>>();

        let entry_notifications = notifications
            .iter()
            .filter(|message| {
                message["params"]["uri"]
                    .as_str()
                    .map(|value| value == uri)
                    .unwrap_or(false)
            })
            .collect::<Vec<_>>();

        assert_eq!(
            entry_notifications.len(),
            1,
            "unexpected publish sequence: {notifications:?}"
        );
        assert!(
            entry_notifications[0]["params"]["diagnostics"]
                .as_array()
                .map(|diagnostics| !diagnostics.is_empty())
                .unwrap_or(false),
            "expected non-empty diagnostics for entry uri: {entry_notifications:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn did_open_publishes_diagnostics_immediately() {
        let dir = mk_temp_dir("did_open_publish");
        let main = dir.join("main.onda");
        let source = "sample:\n  out1 = missing\n";
        write_file(&main, source);

        let mut server = LspServer::default();
        let uri = path_to_file_uri(&main);
        let mut writer = Vec::new();
        let message = json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didOpen",
            "params": {
                "textDocument": {
                    "uri": uri,
                    "version": 1,
                    "text": source,
                }
            }
        });

        server
            .handle_message(message, &mut writer)
            .expect("didOpen should succeed");

        let notifications = decode_lsp_messages(writer)
            .into_iter()
            .filter(|message| {
                message["method"]
                    .as_str()
                    .map(|method| method == "textDocument/publishDiagnostics")
                    .unwrap_or(false)
            })
            .collect::<Vec<_>>();

        assert!(
            notifications.iter().any(|message| {
                message["params"]["diagnostics"]
                    .as_array()
                    .map(|diagnostics| !diagnostics.is_empty())
                    .unwrap_or(false)
            }),
            "expected didOpen to publish diagnostics: {notifications:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn did_open_reports_unselected_buffer_collection_metadata() {
        let dir = mk_temp_dir("did_open_buffer_collection");
        let main = dir.join("main.onda");
        let source = "buffers:\n  bank: f32[] {2}\nblock:\n  channels = bank.chans()\n  sample:\n    out1 = 0.0\n";

        let mut server = LspServer::default();
        let mut writer = Vec::new();
        server
            .handle_message(
                json!({
                    "jsonrpc": "2.0",
                    "method": "textDocument/didOpen",
                    "params": {
                        "textDocument": {
                            "uri": path_to_file_uri(&main),
                            "languageId": "onda",
                            "version": 1,
                            "text": source
                        }
                    }
                }),
                &mut writer,
            )
            .expect("didOpen should succeed");

        let diagnostics = decode_lsp_messages(writer)
            .into_iter()
            .filter(|message| message["method"] == json!("textDocument/publishDiagnostics"))
            .flat_map(|message| {
                message["params"]["diagnostics"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
            })
            .collect::<Vec<_>>();
        assert!(
            diagnostics.iter().any(|diagnostic| diagnostic["message"]
                .as_str()
                .is_some_and(|message| message.contains("select a slot"))),
            "expected collection diagnostic from didOpen: {diagnostics:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn did_open_reports_invalid_integer_lookup_specializations() {
        let dir = mk_temp_dir("did_open_integer_lookup_position");
        let main = dir.join("main.onda");
        let source = "params:\n  layer: i32\n  position: i32\nbuffers:\n  layers: f32[] {4}\nblock:\n  source = layers[layer]\n  sample:\n    out1 = source.readL(0, position)\n";

        let mut server = LspServer::default();
        let mut writer = Vec::new();
        server
            .handle_message(
                json!({
                    "jsonrpc": "2.0",
                    "method": "textDocument/didOpen",
                    "params": {
                        "textDocument": {
                            "uri": path_to_file_uri(&main),
                            "languageId": "onda",
                            "version": 1,
                            "text": source
                        }
                    }
                }),
                &mut writer,
            )
            .expect("didOpen should succeed");

        let diagnostics = decode_lsp_messages(writer)
            .into_iter()
            .find(|message| message["method"] == json!("textDocument/publishDiagnostics"))
            .and_then(|message| message["params"]["diagnostics"].as_array().cloned())
            .unwrap_or_default();
        assert_eq!(
            diagnostics.len(),
            1,
            "unexpected diagnostics: {diagnostics:?}"
        );
        let diagnostic = &diagnostics[0];
        let message = diagnostic["message"].as_str().unwrap_or_default();
        assert!(
            message.contains("while checking specialization of 'std::lookup::_split_position'")
                && message.contains("got I32"),
            "unexpected diagnostic message: {message}"
        );
        assert_eq!(diagnostic["code"], json!("semantic"));
        assert_eq!(diagnostic["range"]["start"]["line"], json!(8));
        assert_eq!(diagnostic["range"]["start"]["character"], json!(11));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn did_open_empty_document_publishes_no_errors() {
        let dir = mk_temp_dir("did_open_empty_document");
        let main = dir.join("empty.onda");

        let mut server = LspServer::default();
        let mut writer = Vec::new();
        server
            .handle_message(
                json!({
                    "jsonrpc": "2.0",
                    "method": "textDocument/didOpen",
                    "params": {
                        "textDocument": {
                            "uri": path_to_file_uri(&main),
                            "languageId": "onda",
                            "version": 1,
                            "text": ""
                        }
                    }
                }),
                &mut writer,
            )
            .expect("didOpen should succeed");

        let notifications = decode_lsp_messages(writer);
        let diagnostics = notifications
            .iter()
            .filter(|message| message["method"] == json!("textDocument/publishDiagnostics"))
            .flat_map(|message| {
                message["params"]["diagnostics"]
                    .as_array()
                    .into_iter()
                    .flatten()
            })
            .collect::<Vec<_>>();
        assert!(diagnostics.is_empty(), "diagnostics: {diagnostics:?}");

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn lsp_hides_missing_sample_diagnostic() {
        let dir = mk_temp_dir("hide_missing_sample_diagnostic");
        let main = dir.join("main.onda");
        let source = "outs:\n  out1\n";
        write_file(&main, source);

        let mut server = LspServer::default();
        let normalized =
            server
                .session
                .open_document(&main, onda_daemon::DocumentVersion(1), source);
        let mut writer = Vec::new();
        server
            .publish_diagnostics_for_entry(&normalized, &mut writer)
            .expect("diagnostics should publish");

        let notifications = decode_lsp_messages(writer);
        let messages = notifications
            .iter()
            .filter(|message| message["method"] == json!("textDocument/publishDiagnostics"))
            .flat_map(|message| {
                message["params"]["diagnostics"]
                    .as_array()
                    .into_iter()
                    .flatten()
            })
            .filter_map(|diagnostic| diagnostic["message"].as_str())
            .collect::<Vec<_>>();

        assert!(
            messages
                .iter()
                .all(|message| !message.contains("missing required 'sample' block")),
            "messages: {messages:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn did_open_runs_diagnostics_even_when_diagnostics_are_deferred() {
        let dir = mk_temp_dir("did_open_deferred_runs_now");
        let main = dir.join("main.onda");
        let source = r#"
import std/delay

proc CyberCell<T>:
  ins<T>:
    src
    fb

  outs<T> 1

  params<T>:
    drive = 1.0
    bias = 0.0

  sample:
    out1 = (src + fb + bias) * drive

init:
  smear = std::delay::Delay<f64>()
"#;
        write_file(&main, source);

        let mut server = LspServer {
            defer_diagnostics: true,
            ..LspServer::default()
        };
        let uri = path_to_file_uri(&main);
        let mut writer = Vec::new();
        server
            .handle_message(
                json!({
                    "jsonrpc": "2.0",
                    "method": "textDocument/didOpen",
                    "params": {
                        "textDocument": {
                            "uri": uri,
                            "version": 1,
                            "text": source,
                        }
                    }
                }),
                &mut writer,
            )
            .expect("didOpen should succeed");

        assert!(
            server.diagnostic_requests.is_empty(),
            "didOpen should run diagnostics instead of scheduling them"
        );
        let normalized = server
            .session
            .open_documents()
            .keys()
            .next()
            .expect("opened document path")
            .clone();
        assert!(
            server
                .parse_cache
                .get(&normalized)
                .and_then(|cached| cached.parsed.as_ref())
                .is_some(),
            "didOpen diagnostics should populate the parsed snapshot"
        );
        let definition = definition_for(&mut server, &main, source, "Delay");
        assert_ne!(
            definition,
            json!(null),
            "definition should use the didOpen-populated parsed snapshot"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn did_save_publishes_diagnostics_immediately() {
        let dir = mk_temp_dir("did_save_publish");
        let main = dir.join("main.onda");
        let valid_source = "sample:\n  out1 = 0.0\n";
        let invalid_source = "sample:\n  out1 = missing\n";
        write_file(&main, valid_source);

        let mut server = LspServer::default();
        let uri = path_to_file_uri(&main);
        server
            .handle_message(
                json!({
                    "jsonrpc": "2.0",
                    "method": "textDocument/didOpen",
                    "params": {
                        "textDocument": {
                            "uri": uri,
                            "version": 1,
                            "text": valid_source,
                        }
                    }
                }),
                &mut Vec::new(),
            )
            .expect("didOpen should succeed");

        let mut writer = Vec::new();
        server
            .handle_message(
                json!({
                    "jsonrpc": "2.0",
                    "method": "textDocument/didSave",
                    "params": {
                        "textDocument": {
                            "uri": uri,
                        },
                        "text": invalid_source,
                    }
                }),
                &mut writer,
            )
            .expect("didSave should succeed");

        let notifications = decode_lsp_messages(writer)
            .into_iter()
            .filter(|message| {
                message["method"]
                    .as_str()
                    .map(|method| method == "textDocument/publishDiagnostics")
                    .unwrap_or(false)
            })
            .collect::<Vec<_>>();

        assert!(
            notifications.iter().any(|message| {
                message["params"]["diagnostics"]
                    .as_array()
                    .map(|diagnostics| !diagnostics.is_empty())
                    .unwrap_or(false)
            }),
            "expected didSave to publish diagnostics: {notifications:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn did_change_publishes_diagnostics_in_synchronous_mode() {
        let dir = mk_temp_dir("did_change_publish_sync");
        let main = dir.join("main.onda");
        let valid_source = "sample:\n  out1 = 0.0\n";
        let invalid_source = "sample:\n  out1 = missing\n";
        write_file(&main, valid_source);

        let mut server = LspServer::default();
        let uri = path_to_file_uri(&main);
        server
            .handle_message(
                json!({
                    "jsonrpc": "2.0",
                    "method": "textDocument/didOpen",
                    "params": {
                        "textDocument": {
                            "uri": uri,
                            "version": 1,
                            "text": valid_source,
                        }
                    }
                }),
                &mut Vec::new(),
            )
            .expect("didOpen should succeed");

        let mut writer = Vec::new();
        server
            .handle_message(
                json!({
                    "jsonrpc": "2.0",
                    "method": "textDocument/didChange",
                    "params": {
                        "textDocument": {
                            "uri": uri,
                            "version": 2,
                        },
                        "contentChanges": [
                            {
                                "text": invalid_source,
                            }
                        ],
                    }
                }),
                &mut writer,
            )
            .expect("didChange should succeed");

        let notifications = decode_lsp_messages(writer)
            .into_iter()
            .filter(|message| {
                message["method"]
                    .as_str()
                    .map(|method| method == "textDocument/publishDiagnostics")
                    .unwrap_or(false)
            })
            .collect::<Vec<_>>();

        assert!(
            notifications.iter().any(|message| {
                message["params"]["diagnostics"]
                    .as_array()
                    .map(|diagnostics| !diagnostics.is_empty())
                    .unwrap_or(false)
            }),
            "didChange should publish diagnostics in synchronous mode: {notifications:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn did_change_defers_diagnostics_in_deferred_mode() {
        let dir = mk_temp_dir("did_change_deferred");
        let main = dir.join("main.onda");
        let valid_source = "sample:\n  out1 = 0.0\n";
        let invalid_source = "sample:\n  out1 = missing\n";
        write_file(&main, valid_source);

        let mut server = LspServer {
            defer_diagnostics: true,
            ..LspServer::default()
        };
        let uri = path_to_file_uri(&main);
        server
            .handle_message(
                json!({
                    "jsonrpc": "2.0",
                    "method": "textDocument/didOpen",
                    "params": {
                        "textDocument": {
                            "uri": uri,
                            "version": 1,
                            "text": valid_source,
                        }
                    }
                }),
                &mut Vec::new(),
            )
            .expect("didOpen should succeed");
        server.take_diagnostic_requests();

        let mut writer = Vec::new();
        server
            .handle_message(
                json!({
                    "jsonrpc": "2.0",
                    "method": "textDocument/didChange",
                    "params": {
                        "textDocument": {
                            "uri": uri,
                            "version": 2,
                        },
                        "contentChanges": [
                            {
                                "text": invalid_source,
                            }
                        ],
                    }
                }),
                &mut writer,
            )
            .expect("didChange should succeed");

        let notifications = decode_lsp_messages(writer)
            .into_iter()
            .filter(|message| {
                message["method"]
                    .as_str()
                    .map(|method| method == "textDocument/publishDiagnostics")
                    .unwrap_or(false)
            })
            .collect::<Vec<_>>();
        assert!(
            notifications.is_empty(),
            "deferred didChange should not write diagnostics on the request path: {notifications:?}"
        );

        let requests = server.take_diagnostic_requests();
        assert!(
            matches!(
                requests.as_slice(),
                [DiagnosticScheduleRequest {
                    delay: DiagnosticDelay::Debounced,
                    ..
                }]
            ),
            "deferred didChange should queue debounced affected-entry diagnostics: {requests:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn deferred_diagnostics_drop_stale_generations() {
        let dir = mk_temp_dir("diagnostic_stale_generation");
        let main = dir.join("main.onda");
        write_file(&main, "sample:\n  out1 = 0.0\n");
        let main = super::normalize_path(&main);

        let (immediate_tx, _immediate_rx) = mpsc::channel();
        let (background_tx, _background_rx) = mpsc::channel();
        let mut core = LspCore::new(immediate_tx, background_tx);
        core.diagnostic_generations.insert(main.clone(), 2);

        let mut writer = Vec::new();
        core.publish_diagnostic_result(
            DiagnosticJobResult {
                entry_path: main,
                generation: 1,
                diagnostics: Vec::new(),
                sources: SourceManifest::default(),
                parse_succeeded: false,
                parse_fingerprint: None,
                completion_index_snapshot: None,
            },
            &mut writer,
        )
        .expect("stale diagnostics should be accepted and ignored");

        assert!(
            decode_lsp_messages(writer).is_empty(),
            "stale diagnostics should not publish notifications"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn deferred_diagnostics_drop_closed_entries() {
        let dir = mk_temp_dir("diagnostic_closed_entry");
        let main = dir.join("main.onda");
        write_file(&main, "sample:\n  out1 = 0.0\n");
        let main = super::normalize_path(&main);

        let (immediate_tx, _immediate_rx) = mpsc::channel();
        let (background_tx, _background_rx) = mpsc::channel();
        let mut core = LspCore::new(immediate_tx, background_tx);
        core.diagnostic_generations.insert(main.clone(), 1);

        let mut writer = Vec::new();
        core.publish_diagnostic_result(
            DiagnosticJobResult {
                entry_path: main,
                generation: 1,
                diagnostics: Vec::new(),
                sources: SourceManifest::default(),
                parse_succeeded: false,
                parse_fingerprint: None,
                completion_index_snapshot: None,
            },
            &mut writer,
        )
        .expect("closed-entry diagnostics should be accepted and ignored");

        assert!(
            decode_lsp_messages(writer).is_empty(),
            "closed-entry diagnostics should not publish notifications"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn deferred_diagnostic_parse_keeps_source_files_for_navigation() {
        let dir = mk_temp_dir("diagnostic_parse_source_files");
        let main = super::normalize_path(&dir.join("main.onda"));
        let lib = super::normalize_path(&dir.join("lib.onda"));
        let main_source = "import lib\n\nsample:\n  out1 = Target\n";
        let lib_source = "const Target = 1.0\n";
        write_file(&main, main_source);
        write_file(&lib, lib_source);

        // Source locations use compact thread-local IDs. Occupy the IDs that
        // the diagnostic worker will independently assign to this source graph
        // so a worker-owned Program cannot accidentally appear valid here.
        let _ = onda_frontend::SourceLoc::new(
            Some(dir.join("unrelated_one.onda").display().to_string()),
            1,
            1,
            1,
            2,
            Vec::new(),
        );
        let _ = onda_frontend::SourceLoc::new(
            Some(dir.join("unrelated_two.onda").display().to_string()),
            1,
            1,
            1,
            2,
            Vec::new(),
        );

        let worker_main = main.clone();
        let worker_source = main_source.to_owned();
        let result = std::thread::spawn(move || {
            super::run_diagnostic_job(super::DiagnosticJob {
                entry_path: worker_main.clone(),
                generation: 1,
                open_documents: vec![super::DiagnosticOpenDocument {
                    path: worker_main,
                    version: onda_daemon::DocumentVersion(1),
                    text: worker_source,
                }],
            })
        })
        .join()
        .expect("diagnostic worker should finish");
        assert!(result.parse_succeeded, "worker source should parse");

        let mut server = LspServer::default();
        server
            .session
            .open_document(&main, onda_daemon::DocumentVersion(1), main_source);
        server
            .publish_diagnostic_result(result, &mut Vec::new())
            .expect("diagnostic result should publish");

        let definition = definition_for(&mut server, &main, main_source, "Target");
        let hover = hover_markdown_for(&mut server, &main, main_source, "Target")
            .expect("imported definition should have hover information");
        assert_eq!(
            definition["uri"],
            json!(path_to_file_uri(&lib)),
            "imported definition should retain the worker's source file: {definition:?}"
        );
        assert!(
            hover.contains(&lib.display().to_string()),
            "hover should name the imported definition's source file: {hover}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn immediate_diagnostics_use_immediate_worker_lane() {
        let dir = mk_temp_dir("diagnostic_immediate_lane");
        let main = super::normalize_path(&dir.join("main.onda"));
        write_file(&main, "sample:\n  out1 = 0.0\n");

        let (immediate_tx, immediate_rx) = mpsc::channel();
        let (background_tx, background_rx) = mpsc::channel();
        let mut core = LspCore::new(immediate_tx, background_tx);
        core.schedule_diagnostic_entries(vec![main.clone()], DiagnosticDelay::Immediate);
        core.dispatch_due_diagnostics();

        let job = immediate_rx
            .try_recv()
            .expect("immediate diagnostics should dispatch to immediate lane");
        assert_eq!(job.entry_path, main);
        assert!(
            background_rx.try_recv().is_err(),
            "immediate diagnostics should not wait in the background lane"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_hides_private_imported_use_symbols() {
        let dir = mk_temp_dir("completion_private_use");
        let lib = dir.join("lib.onda");
        let main = dir.join("main.onda");
        write_file(
            &lib,
            r#"
namespace helpers:
  def shape(x):
    return x

use helpers

def shaped(x):
  return shape(x)
"#,
        );
        let source = r#"
import lib

outs:
  out1

sample:
  out1 = sh
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let labels = completion_labels_for(&mut server, &main, source, "sh");

        assert!(labels.contains(&"shaped".to_owned()), "labels: {labels:?}");
        assert!(
            !labels.contains(&"shape".to_owned()),
            "private imported use should not be completed in importer: {labels:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_reexports_pub_use_symbols() {
        let dir = mk_temp_dir("completion_pub_use");
        let lib = dir.join("lib.onda");
        let main = dir.join("main.onda");
        write_file(
            &lib,
            r#"
namespace helpers:
  def shape(x):
    return x

pub use helpers
"#,
        );
        let source = r#"
import lib

outs:
  out1

sample:
  out1 = sh
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let labels = completion_labels_for(&mut server, &main, source, "sh");

        assert!(
            labels.contains(&"shape".to_owned()),
            "pub use should be completed in importer: {labels:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_filters_private_proc_params_after_receiver_dot() {
        let dir = mk_temp_dir("completion_proc_members");
        let main = dir.join("main.onda");
        let source = r#"
proc Voice:
  params:
    private cutoff = 1000.0
    gain = 1.0

  outs:
    out1

  events:
    note_on(v):
      gain = v

  sample:
    out1 = gain

init:
  voice = Voice()

sample:
  voice.
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let labels = completion_labels_for(&mut server, &main, source, "voice.");

        assert!(labels.contains(&"gain".to_owned()), "labels: {labels:?}");
        assert!(labels.contains(&"out1".to_owned()), "labels: {labels:?}");
        assert!(labels.contains(&"note_on".to_owned()), "labels: {labels:?}");
        assert!(labels.contains(&"init".to_owned()), "labels: {labels:?}");
        assert!(
            !labels.contains(&"cutoff".to_owned()),
            "private param should not be exposed after receiver dot: {labels:?}"
        );
        assert!(
            !labels.contains(&"params".to_owned()),
            "dynamic params should be hidden when a proc has private params: {labels:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_hides_dynamic_params_for_procs_without_params() {
        let dir = mk_temp_dir("completion_proc_no_params");
        let main = dir.join("main.onda");
        let source = r#"
proc Voice:
  outs:
    out1

  sample:
    out1 = 0.0

init:
  voice = Voice()

sample:
  voice.
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let labels = completion_labels_for(&mut server, &main, source, "voice.");

        assert!(labels.contains(&"out1".to_owned()), "labels: {labels:?}");
        assert!(labels.contains(&"init".to_owned()), "labels: {labels:?}");
        assert!(
            !labels.contains(&"params".to_owned()),
            "dynamic params should be hidden when a proc declares no params: {labels:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_does_not_leak_instances_from_other_scopes() {
        let dir = mk_temp_dir("completion_instance_scope_leak");
        let main = dir.join("main.onda");
        let source = r#"
proc Voice:
  params:
    gain = 1.0

  outs:
    out1

  sample:
    out1 = gain

def build():
  v = Voice()
  return 0.0

outs:
  out1

sample:
  v.
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let labels = completion_labels_for(&mut server, &main, source, "v.");

        assert!(
            !labels.contains(&"gain".to_owned()),
            "function-local instance should not leak into sample member completion: {labels:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_filters_private_proc_params_in_live_call_args() {
        let dir = mk_temp_dir("completion_proc_call_args");
        let main = dir.join("main.onda");
        let source = r#"
proc Voice:
  params:
    private cutoff = 1000.0
    gain = 1.0

  outs:
    out1

  sample:
    out1 = gain

init:
  voice = Voice()

sample:
  out1 = voice(
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let labels = completion_labels_for(&mut server, &main, source, "voice(");

        assert!(labels.contains(&"gain".to_owned()), "labels: {labels:?}");
        assert!(
            !labels.contains(&"cutoff".to_owned()),
            "private param should not be exposed as a live call arg: {labels:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_lists_top_level_runtime_symbols_in_sample() {
        let dir = mk_temp_dir("completion_top_level_runtime");
        let main = dir.join("main.onda");
        let source = r#"
params:
  gain = 1.0

outs:
  out1

init:
  phase = 0.0

sample:
  ga
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let labels = completion_labels_for(&mut server, &main, source, "ga");

        assert!(labels.contains(&"gain".to_owned()), "labels: {labels:?}");

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_lists_proc_scope_symbols_in_local_defs() {
        let dir = mk_temp_dir("completion_proc_scope_symbols");
        let main = dir.join("main.onda");
        let source = r#"
proc Voice:
  ins:
    dry

  params:
    gain = 1.0

  outs:
    out1

  init:
    cached = 0.0

  def update(delta):
    ga

  sample:
    out1 = gain
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let labels = completion_labels_for(&mut server, &main, source, "ga");

        assert!(labels.contains(&"gain".to_owned()), "labels: {labels:?}");

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_does_not_leak_other_function_locals() {
        let dir = mk_temp_dir("completion_scope_leak");
        let main = dir.join("main.onda");
        let source = r#"
def other():
  secret = 1
  return secret

def current():
  return se
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let labels = completion_labels_for(&mut server, &main, source, "se");

        assert!(
            !labels.contains(&"secret".to_owned()),
            "locals from sibling defs should not be completed: {labels:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_does_not_leak_control_flow_locals() {
        let dir = mk_temp_dir("completion_control_flow_scope");
        let main = dir.join("main.onda");
        let source = r#"
outs:
  out1

sample:
  if true:
    tmp = 1.0
  for i in 0..2:
    loop_tmp = f32(i)
  out1 = t
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let labels = completion_labels_for(&mut server, &main, source, "out1 = t");

        assert!(
            !labels.contains(&"tmp".to_owned()),
            "branch local should not complete after branch: {labels:?}"
        );
        assert!(
            !labels.contains(&"loop_tmp".to_owned()) && !labels.contains(&"i".to_owned()),
            "loop locals should not complete after loop: {labels:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_does_not_offer_future_locals() {
        let dir = mk_temp_dir("completion_future_local");
        let main = dir.join("main.onda");
        let source = r#"
outs:
  out1

sample:
  earlier = 1.0
  out1 =
  later = earlier
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let labels = completion_labels_for(&mut server, &main, source, "out1 =");

        assert!(
            !labels.contains(&"later".to_owned()),
            "future local should not complete before declaration: {labels:?}"
        );
        assert!(
            labels.contains(&"earlier".to_owned()),
            "earlier local should still complete: {labels:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_hides_generated_onda_symbols() {
        let dir = mk_temp_dir("completion_generated_onda_symbols");
        let main = dir.join("main.onda");
        let source = r#"
outs:
  out1

sample:
  visible = 1.0
  __onda_internal = 2.0
  out1 =
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let labels = completion_labels_for(&mut server, &main, source, "out1 =");

        assert!(
            labels.contains(&"visible".to_owned()),
            "ordinary local should complete: {labels:?}"
        );
        assert!(
            labels.iter().all(|label| !label.starts_with("__onda")),
            "generated/internal symbols should not complete: {labels:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_does_not_duplicate_init_state_as_local() {
        let dir = mk_temp_dir("completion_init_state_dedup");
        let main = dir.join("main.onda");
        let source = r#"
outs:
  out1

init:
  phase = 0.0
  ph

sample:
  out1 = phase
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let items = completion_items_for(&mut server, &main, source, "  ph");
        let phase_details = items
            .iter()
            .filter(|item| item["label"] == json!("phase"))
            .map(|item| item["detail"].as_str().unwrap_or_default().to_owned())
            .collect::<Vec<_>>();

        assert_eq!(
            phase_details,
            vec!["state".to_owned()],
            "init state should complete once as state: {items:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_lists_namespace_members_on_incomplete_line() {
        let dir = mk_temp_dir("completion_namespace_members");
        let main = dir.join("main.onda");
        let source = r#"
import std/osc

outs:
  out1

sample:
  out1 = std::osc::
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let labels = completion_labels_for(&mut server, &main, source, "std::osc::");

        assert!(labels.contains(&"Sine".to_owned()), "labels: {labels:?}");
        assert!(labels.contains(&"Saw".to_owned()), "labels: {labels:?}");

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_discovers_std_namespaces_without_prior_import() {
        let dir = mk_temp_dir("completion_std_namespace_discovery");
        let main = dir.join("main.onda");
        let source = "init:\n  a = std::\n";
        write_file(&main, source);

        let mut server = LspServer::default();
        let items = completion_items_for(&mut server, &main, source, "std::");
        let labels = items
            .iter()
            .filter_map(|item| item["label"].as_str())
            .collect::<Vec<_>>();

        for expected in ["osc", "filter", "env", "delay", "dynamics", "sample"] {
            assert!(labels.contains(&expected), "missing {expected}: {items:?}");
        }
        for unrelated in ["init", "sin", "PI"] {
            assert!(
                !labels.contains(&unrelated),
                "unexpected {unrelated}: {items:?}"
            );
        }
        assert!(
            items.iter().all(|item| item["kind"] == json!(9)),
            "std:: should contain only namespaces: {items:?}"
        );
        assert!(
            items.iter().all(|item| {
                item["sortText"]
                    .as_str()
                    .is_some_and(|sort_text| sort_text.starts_with("00_00_"))
            }),
            "namespace ranks: {items:?}"
        );

        let osc = items
            .iter()
            .find(|item| item["label"] == json!("osc"))
            .expect("osc namespace completion");
        assert_eq!(
            osc["additionalTextEdits"][0]["newText"],
            json!("import std/osc\n"),
            "item: {osc:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_filters_std_namespace_prefix_without_prior_import() {
        let dir = mk_temp_dir("completion_std_namespace_prefix");
        let main = dir.join("main.onda");
        let source = "init:\n  a = std::o\n";
        write_file(&main, source);

        let mut server = LspServer::default();
        let labels = completion_labels_for(&mut server, &main, source, "std::o");

        assert_eq!(labels, vec!["osc".to_owned()], "labels: {labels:?}");

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_discovers_std_module_symbols_without_prior_import() {
        let dir = mk_temp_dir("completion_std_module_symbol_discovery");
        let main = dir.join("main.onda");
        let source = "init:\n  a = std::osc::\n";
        write_file(&main, source);

        let mut server = LspServer::default();
        let items = completion_items_for(&mut server, &main, source, "std::osc::");
        let labels = items
            .iter()
            .filter_map(|item| item["label"].as_str())
            .collect::<Vec<_>>();

        for expected in ["Phasor", "Sine", "Saw", "Pulse"] {
            assert!(labels.contains(&expected), "missing {expected}: {items:?}");
        }
        for unrelated in ["init", "sample", "sin", "PI", "Complex"] {
            assert!(
                !labels.contains(&unrelated),
                "unexpected {unrelated}: {items:?}"
            );
        }

        assert_eq!(
            &labels[..8],
            ["KSine", "Phasor", "Pulse", "Saw", "SawDown", "Sine", "Square", "Triangle"],
            "constructors should lead the qualified list: {items:?}"
        );
        assert!(
            items[..8].iter().all(|item| {
                item["sortText"]
                    .as_str()
                    .is_some_and(|sort_text| sort_text.starts_with("00_01_"))
            }),
            "constructor ranks: {items:?}"
        );

        let sine = items
            .iter()
            .find(|item| item["label"] == json!("Sine"))
            .expect("Sine completion");
        assert_eq!(sine["kind"], json!(4), "item: {sine:?}");
        assert_eq!(
            sine["additionalTextEdits"][0]["newText"],
            json!("import std/osc\n"),
            "item: {sine:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_does_not_duplicate_existing_std_import_edit() {
        let dir = mk_temp_dir("completion_existing_std_import");
        let main = dir.join("main.onda");
        let source = "import std/osc\ninit:\n  a = std::osc::\n";
        write_file(&main, source);

        let mut server = LspServer::default();
        let items = completion_items_for(&mut server, &main, source, "std::osc::");
        let sine = items
            .iter()
            .find(|item| item["label"] == json!("Sine"))
            .expect("Sine completion");

        assert!(
            sine.get("additionalTextEdits").is_none(),
            "existing import should not be duplicated: {sine:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_replaces_general_session_across_std_namespace_triggers() {
        let dir = mk_temp_dir("completion_std_trigger_sequence");
        let main = dir.join("main.onda");
        let mut server = LspServer::default();

        let root_source = "init:\n  a = std::\n";
        write_file(&main, root_source);
        let root_labels = completion_items_for_with_context(
            &mut server,
            &main,
            root_source,
            "std::",
            Some(json!({
                "triggerKind": 2,
                "triggerCharacter": ":",
            })),
        )
        .iter()
        .filter_map(|item| item["label"].as_str().map(ToOwned::to_owned))
        .collect::<Vec<_>>();
        assert!(
            root_labels.contains(&"osc".to_owned()),
            "labels: {root_labels:?}"
        );
        assert!(
            !root_labels.contains(&"Sine".to_owned()),
            "labels: {root_labels:?}"
        );

        let module_source = "init:\n  a = std::osc::\n";
        let module_labels = completion_items_for_with_context(
            &mut server,
            &main,
            module_source,
            "std::osc::",
            Some(json!({
                "triggerKind": 2,
                "triggerCharacter": ":",
            })),
        )
        .iter()
        .filter_map(|item| item["label"].as_str().map(ToOwned::to_owned))
        .collect::<Vec<_>>();
        assert!(
            module_labels.contains(&"Sine".to_owned()),
            "labels: {module_labels:?}"
        );
        assert!(
            !module_labels.contains(&"sample".to_owned()),
            "labels: {module_labels:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_marks_empty_qualified_prefix_incomplete_for_requery() {
        let dir = mk_temp_dir("completion_incomplete_requery");
        let main = dir.join("main.onda");
        let source = "import std/osc\ninit:\n  sine = std::osc::\n";
        write_file(&main, source);

        let mut server = LspServer::default();
        let normalized =
            server
                .session
                .open_document(&main, onda_daemon::DocumentVersion(1), source);
        let result = server
            .completions_for_uri(
                &path_to_file_uri(&normalized),
                position_after(source, "std::osc::"),
                None,
            )
            .expect("completion should succeed");

        assert_eq!(result["isIncomplete"], json!(true), "result: {result:?}");
        assert!(
            result["items"]
                .as_array()
                .is_some_and(|items| items.iter().any(|item| item["label"] == json!("Sine"))),
            "result: {result:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_lists_only_std_osc_members_on_first_qualified_request() {
        let dir = mk_temp_dir("completion_std_osc_first_qualified_request");
        let main = dir.join("main.onda");
        write_file(&main, "");

        let mut server = LspServer::default();
        let normalized = server
            .session
            .open_document(&main, onda_daemon::DocumentVersion(1), "");
        server
            .publish_diagnostics_for_entry(&normalized, &mut Vec::new())
            .expect("empty document diagnostics should populate the initial parse cache");

        let source = r#"import std/osc

init:
  sine = std::osc::Si
"#;
        server
            .session
            .update_document(&main, onda_daemon::DocumentVersion(2), source);
        server.note_document_changed(&main);

        let items = completion_items_for(&mut server, &main, source, "std::osc::Si");

        assert_eq!(items.len(), 1, "items: {items:?}");
        assert_eq!(items[0]["label"], json!("Sine"), "item: {:?}", items[0]);
        assert_eq!(items[0]["kind"], json!(4), "item: {:?}", items[0]);
        assert!(
            items[0]["detail"]
                .as_str()
                .is_some_and(|detail| detail.starts_with("proc std::osc::Sine")),
            "item: {:?}",
            items[0]
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_for_bare_std_osc_namespace_excludes_unrelated_symbols() {
        let dir = mk_temp_dir("completion_std_osc_namespace_only");
        let main = dir.join("main.onda");
        let source = r#"import std/osc

init:
  sine = std::osc::
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let labels = completion_labels_for(&mut server, &main, source, "std::osc::");

        for expected in ["Phasor", "Sine", "Saw", "poly_blep"] {
            assert!(
                labels.contains(&expected.to_owned()),
                "missing {expected}: {labels:?}"
            );
        }
        for unrelated in ["sin", "PI", "sample", "Complex"] {
            assert!(
                !labels.contains(&unrelated.to_owned()),
                "unexpected {unrelated}: {labels:?}"
            );
        }

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_after_import_slash_inserts_only_the_remaining_segment() {
        let dir = mk_temp_dir("completion_import_path_segment");
        let main = dir.join("main.onda");
        let source = "import std/";
        write_file(&main, source);

        let mut server = LspServer::default();
        let items = completion_items_for(&mut server, &main, source, "std/");
        let osc = items
            .iter()
            .find(|item| item["label"] == json!("osc"))
            .unwrap_or_else(|| panic!("missing osc module completion: {items:?}"));

        assert_eq!(osc["insertText"], json!("osc"), "item: {osc:?}");
        assert!(
            items
                .iter()
                .all(|item| item["insertText"] != json!("std/osc")),
            "completion must not duplicate the existing std/ prefix: {items:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_does_not_resolve_unqualified_imported_namespace_types() {
        let dir = mk_temp_dir("completion_unqualified_imported_namespace_type");
        let main = dir.join("main.onda");
        let source = r#"import std/osc

init:
  sine = Sine()
  sine.
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let labels = completion_labels_for(&mut server, &main, source, "sine.");

        assert!(labels.is_empty(), "labels: {labels:?}");

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_does_not_expose_std_members_through_use_without_import() {
        let dir = mk_temp_dir("completion_std_use_without_import");
        let main = dir.join("main.onda");
        let source = r#"use std::osc

init:
  sine = Si
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let labels = completion_labels_for(&mut server, &main, source, "Si");

        assert!(
            !labels.contains(&"Sine".to_owned()),
            "use must not expose members of an unimported module: {labels:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn navigation_does_not_resolve_members_of_unqualified_imported_namespace_types() {
        let dir = mk_temp_dir("navigation_unqualified_imported_namespace_type");
        let main = dir.join("main.onda");
        let source = r#"import std/osc

init:
  sine = Sine()
  value = sine.freq
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let definition = definition_for(&mut server, &main, source, "sine.freq");

        assert_eq!(definition, json!(null), "definition: {definition:?}");

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_resolves_unqualified_namespace_types_after_use() {
        let dir = mk_temp_dir("completion_used_namespace_type");
        let main = dir.join("main.onda");
        let source = r#"import std/osc
use std::osc

init:
  sine = Sine()
  sine.
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let labels = completion_labels_for(&mut server, &main, source, "sine.");

        assert!(labels.contains(&"freq".to_owned()), "labels: {labels:?}");
        assert!(labels.contains(&"amp".to_owned()), "labels: {labels:?}");

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_lists_members_after_std_generic_namespace() {
        let dir = mk_temp_dir("completion_std_generic_namespace_members");
        let main = dir.join("main.onda");
        let source = r#"
import std/convolution

namespace Test<FFTSize = 64, MaxImpulseLen = 1024>:
  proc Use<T>:
    init:
      conv = std::convolution<FFTSize, MaxImpulseLen>::
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let labels = completion_labels_for(
            &mut server,
            &main,
            source,
            "std::convolution<FFTSize, MaxImpulseLen>::",
        );

        assert!(
            labels.contains(&"BlockConvolver".to_owned()),
            "labels: {labels:?}"
        );
        assert!(
            labels.contains(&"TimeDomainConvolver".to_owned()),
            "labels: {labels:?}"
        );
        assert!(
            labels.contains(&"ZeroLatencyConvolver".to_owned()),
            "labels: {labels:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_resolves_relative_generic_namespace_members_from_current_scope() {
        let dir = mk_temp_dir("completion_relative_generic_namespace_members");
        let main = dir.join("main.onda");
        let source = r#"
namespace sc:
  namespace Convolution2<FFTSize = 64, MaxKernel = 1024>:
    namespace Mono:
      proc ar<T>:
        outs<T> 1
        sample:
          out1 = 0.0

  namespace Convolution3<FFTSize = 64, MaxKernel = 1024>:
    namespace Mono:
      proc ar<T>:
        init:
          c = Convolution2<FFTSize, MaxKernel>::
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let labels = completion_labels_for(
            &mut server,
            &main,
            source,
            "Convolution2<FFTSize, MaxKernel>::",
        );

        assert!(labels.contains(&"Mono".to_owned()), "labels: {labels:?}");

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_lists_current_namespace_symbols_unqualified() {
        let dir = mk_temp_dir("completion_current_namespace");
        let main = dir.join("main.onda");
        let source = r#"
namespace DSP:
  const Bias = 0.5

  struct Shape:
    x: f32

  proc Voice:
    outs:
      out1
    sample:
      out1 = 0.0

  namespace Inner:
    const X = 1

  def shape(x):
    return x

  def run(x):
    return (
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let labels = completion_labels_for(&mut server, &main, source, "return (");

        assert!(labels.contains(&"Bias".to_owned()), "labels: {labels:?}");
        assert!(labels.contains(&"Shape".to_owned()), "labels: {labels:?}");
        assert!(labels.contains(&"Voice".to_owned()), "labels: {labels:?}");
        assert!(labels.contains(&"Inner".to_owned()), "labels: {labels:?}");
        assert!(labels.contains(&"shape".to_owned()), "labels: {labels:?}");

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_lists_child_namespaces_from_single_namespace_use() {
        let dir = mk_temp_dir("completion_single_namespace_use_children");
        let main = dir.join("main.onda");
        let source = r#"
namespace sc:
  namespace SinOsc:
    proc ar:
      outs:
        out1
      sample:
        out1 = 0.0

use sc

init:
  a = SinO

sample:
  out1 = 0.0
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let labels = completion_labels_for(&mut server, &main, source, "SinO");

        assert!(labels.contains(&"SinOsc".to_owned()), "labels: {labels:?}");

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_walks_child_namespace_from_single_namespace_use() {
        let dir = mk_temp_dir("completion_single_namespace_use_walk");
        let main = dir.join("main.onda");
        let source = r#"
namespace sc:
  namespace SinOsc:
    proc ar:
      outs:
        out1
      sample:
        out1 = 0.0
    proc kr:
      kouts:
        kout1
      block:
        kout1 = 0.0

use sc

init:
  a = SinOsc::a

sample:
  out1 = 0.0
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let all_labels = completion_labels_for(&mut server, &main, source, "SinOsc::");
        let ar_labels = completion_labels_for(&mut server, &main, source, "SinOsc::a");

        assert!(
            all_labels.contains(&"ar".to_owned()),
            "labels: {all_labels:?}"
        );
        assert!(
            all_labels.contains(&"kr".to_owned()),
            "labels: {all_labels:?}"
        );
        assert!(
            ar_labels.contains(&"ar".to_owned()),
            "labels: {ar_labels:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_walks_child_namespace_alias_from_single_namespace_use() {
        let dir = mk_temp_dir("completion_single_namespace_use_alias_walk");
        let main = dir.join("main.onda");
        let source = r#"
namespace ugens:
  namespace LocalOsc:
    def ar():
      return 0.0
    def kr():
      return 0.0

  namespace Osc = LocalOsc

use ugens

init:
  a = Osc::a

sample:
  out1 = 0.0
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let labels = completion_labels_for(&mut server, &main, source, "Osc::a");

        assert!(labels.contains(&"ar".to_owned()), "labels: {labels:?}");

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn definition_walks_child_namespace_from_single_namespace_use() {
        let dir = mk_temp_dir("definition_single_namespace_use_walk");
        let main = dir.join("main.onda");
        let source = r#"
namespace sc:
  namespace SinOsc:
    def ar():
      return 0.0

use sc

sample:
  out1 = SinOsc::ar()
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let definition = definition_for(&mut server, &main, source, "SinOsc::ar");

        assert_ne!(
            definition,
            json!(null),
            "definition should resolve through single namespace use"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn definition_walks_child_namespace_alias_from_single_namespace_use() {
        let dir = mk_temp_dir("definition_single_namespace_use_alias_walk");
        let main = dir.join("main.onda");
        let source = r#"
namespace ugens:
  namespace LocalOsc:
    def ar():
      return 0.0

  namespace Osc = LocalOsc

use ugens

sample:
  out1 = Osc::ar()
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let definition = definition_for(&mut server, &main, source, "Osc::ar");

        assert_ne!(
            definition,
            json!(null),
            "definition should resolve through namespace alias from single namespace use"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn definition_resolves_struct_fields_through_lsp_server_path() {
        let dir = mk_temp_dir("definition_struct_fields_lsp");
        let main = dir.join("main.onda");
        let source = r#"import std/math

struct Box:
  value: f32 = 0.0

  def set(self, target):
    self.value = 0.0

  def get(self):
    return self.value
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let field_definition = definition_for(&mut server, &main, source, "  value");
        let self_definition = definition_for(&mut server, &main, source, "self.value");

        assert_eq!(
            field_definition["range"]["start"]["line"],
            json!(3),
            "field declaration should resolve through server path: {field_definition:?}"
        );
        assert_eq!(
            self_definition["range"]["start"]["line"],
            json!(3),
            "self.field should resolve through server path: {self_definition:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn hover_resolves_struct_fields_through_lsp_server_path() {
        let dir = mk_temp_dir("hover_struct_fields_lsp");
        let main = dir.join("main.onda");
        let source = r#"import std/math

struct Box:
  value: f32 = 0.0

  def set(self, target):
    self.value = 0.0
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let field_hover =
            hover_markdown_for(&mut server, &main, source, "  value").unwrap_or_default();
        let self_hover =
            hover_markdown_for(&mut server, &main, source, "self.value").unwrap_or_default();

        assert!(
            field_hover.contains("field value"),
            "field declaration hover should resolve through server path: {field_hover:?}"
        );
        assert!(
            self_hover.contains("field value"),
            "self.field hover should resolve through server path: {self_hover:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn navigation_resolves_current_struct_fields_with_stale_parse_cache() {
        let dir = mk_temp_dir("navigation_struct_fields_stale_parse");
        let main = super::normalize_path(&dir.join("main.onda"));
        let old_source = r#"import std/math

struct Box:
  old_value: f32 = 0.0

  def get(self):
    return self.old_value
"#;
        let current_source = r#"import std/math

struct Box:
  value: f32 = 0.0

  def get(self):
    return self.value
"#;
        write_file(&main, old_source);

        let mut server = LspServer::default();
        let old_parsed =
            onda_frontend::parse_program_file_with_overlays(&main, &server.session.overlay_map())
                .expect("old source should parse");
        server.cache_parsed_program_for_path(
            &main,
            super::source_fingerprint(old_source),
            Some(old_parsed),
        );
        write_file(&main, current_source);

        let field_definition = definition_for(&mut server, &main, current_source, "  value");
        let self_definition = definition_for(&mut server, &main, current_source, "self.value");
        let self_hover = hover_markdown_for(&mut server, &main, current_source, "self.value")
            .unwrap_or_default();

        assert_eq!(
            field_definition["range"]["start"]["line"],
            json!(3),
            "current field declaration should resolve despite stale parse: {field_definition:?}"
        );
        assert_eq!(
            self_definition["range"]["start"]["line"],
            json!(3),
            "current self.field should resolve despite stale parse: {self_definition:?}"
        );
        assert!(
            self_hover.contains("field value"),
            "current self.field hover should resolve despite stale parse: {self_hover:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn navigation_resolves_struct_methods_and_typed_param_members_through_lsp_server_path() {
        let dir = mk_temp_dir("navigation_struct_methods_lsp");
        let main = dir.join("main.onda");
        let source = r#"
struct Box:
  value: f32 = 0.0

  def set(self, target):
    self.value = 0.0

  def get(self):
    return self.value

  def bump(self, amount):
    self.set(target = amount)

def read(item: Box):
  return item.value + item.get()
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let self_method = definition_for(&mut server, &main, source, "self.set");
        let self_method_arg = definition_for(&mut server, &main, source, "target");
        let param_field = definition_for(&mut server, &main, source, "item.value");
        let param_method = definition_for(&mut server, &main, source, "item.get");

        assert_eq!(self_method["range"]["start"]["line"], json!(4));
        assert_eq!(self_method_arg["range"]["start"]["line"], json!(4));
        assert_eq!(param_field["range"]["start"]["line"], json!(2));
        assert_eq!(param_method["range"]["start"]["line"], json!(7));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn navigation_resolves_struct_constructor_field_args_through_lsp_server_path() {
        let dir = mk_temp_dir("navigation_struct_ctor_args_lsp");
        let main = dir.join("main.onda");
        let source = r#"
struct Pair:
  left: f32 = 0.0
  right: f32 = 0.0

init:
  p = Pair(left = 1.0, right = 2.0)
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let left = definition_for(&mut server, &main, source, "left");
        let right = definition_for(&mut server, &main, source, "right");

        assert_eq!(left["range"]["start"]["line"], json!(2));
        assert_eq!(right["range"]["start"]["line"], json!(3));

        fs::remove_dir_all(&dir).ok();
    }

