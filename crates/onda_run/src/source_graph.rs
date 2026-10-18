use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use onda_frontend::{absolute_lexical_path, load_program_file, SourceManifest};
use onda_project::{
    is_project_file_path, resolve_project_watch_snapshot, FileReadDigest, FileReadDigests,
    ProjectLimits,
};
use serde::Serialize;
use sha2::{Digest, Sha256};

/// Files in the compiler's source graph, plus the project manifest and assets.
/// Incomplete discovery includes unresolved imports; hosts must retain previous
/// dependencies until a complete graph replaces them. Hosts must validate the
/// captured SHA-256 content hashes after installing their filesystem watches.
#[derive(Debug, Clone, Serialize)]
pub struct RunSourceGraph {
    pub files: Vec<PathBuf>,
    pub complete: bool,
    #[serde(rename = "contentHashes")]
    pub content_hashes: BTreeMap<PathBuf, Option<String>>,
}

impl RunSourceGraph {
    /// Publishes a startup source observation, including failed build attempts.
    pub fn write_event(&self, writer: &mut impl Write) -> Result<(), String> {
        let mut graph = serde_json::to_value(self)
            .map_err(|error| format!("failed to encode source graph: {error}"))?;
        graph["event"] = serde_json::json!("sources");
        serde_json::to_writer(&mut *writer, &graph)
            .map_err(|error| format!("failed to write source graph: {error}"))?;
        writeln!(writer)
            .and_then(|_| writer.flush())
            .map_err(|error| format!("failed to flush source graph: {error}"))
    }

    /// Builds a graph from the exact source, manifest, and asset reads used by a run.
    pub fn from_compiled_sources(sources: &SourceManifest, file_digests: FileReadDigests) -> Self {
        Self::from_sources(sources, file_digests, true)
    }

    /// Failed builds retain previous dependencies until discovery succeeds.
    pub fn from_failed_sources(sources: &SourceManifest, file_digests: FileReadDigests) -> Self {
        Self::from_sources(sources, file_digests, false)
    }

    fn from_sources(
        sources: &SourceManifest,
        file_digests: FileReadDigests,
        complete: bool,
    ) -> Self {
        let mut content_hashes = source_hashes(sources);
        let mut files = content_hashes
            .keys()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();
        for (path, digest) in file_digests {
            files.insert(path.clone());
            match digest {
                FileReadDigest::Content(digest) => {
                    content_hashes.insert(path, Some(format_digest(digest)));
                }
                FileReadDigest::Unavailable => {
                    content_hashes.insert(path, None);
                }
                FileReadDigest::Unchecked => {
                    content_hashes.remove(&path);
                }
            }
        }
        Self {
            files: files.into_iter().collect(),
            complete,
            content_hashes,
        }
    }
}

/// Uses the same frontend manifest and project recovery paths as native Run.
/// Discovery does not require a successful compilation or an audio device.
pub fn run_source_graph(input: &Path) -> RunSourceGraph {
    let input = absolute_lexical_path(input).unwrap_or_else(|_| input.to_path_buf());
    let is_project = is_project_file_path(&input);
    let mut file_digests = FileReadDigests::new();
    let project = if is_project {
        let snapshot = resolve_project_watch_snapshot(&input, ProjectLimits::default());
        file_digests.insert(input.clone(), snapshot.manifest_digest);
        snapshot.paths.ok()
    } else {
        None
    };
    if is_project && project.is_none() {
        return RunSourceGraph::from_failed_sources(&SourceManifest::default(), file_digests);
    }
    let entry = project
        .as_ref()
        .map_or(input.as_path(), |paths| paths.entry.as_path());
    let (sources, complete) = match load_program_file(entry) {
        Ok(loaded) => (loaded.sources, true),
        Err(error) => (error.sources, false),
    };
    if sources.files.is_empty() {
        file_digests.insert(entry.to_path_buf(), FileReadDigest::Unavailable);
    }
    if let Some(project) = project {
        // The canonical manifest may have a different lexical spelling.
        let manifest_digest = file_digests[&input];
        file_digests.insert(project.manifest, manifest_digest);
        for path in project.assets {
            let digest = super::digest_file(&path)
                .map_or(FileReadDigest::Unavailable, FileReadDigest::Content);
            file_digests.insert(path, digest);
        }
    }
    RunSourceGraph::from_sources(&sources, file_digests, complete)
}

fn source_hashes(sources: &SourceManifest) -> BTreeMap<PathBuf, Option<String>> {
    let mut hashes = sources
        .documents
        .iter()
        .map(|document| {
            (
                document.path.clone(),
                Some(content_hash(document.contents.as_bytes())),
            )
        })
        .collect::<BTreeMap<_, _>>();
    hashes.extend(
        sources
            .failed_reads
            .iter()
            .map(|(path, contents)| (path.clone(), contents.as_deref().map(content_hash))),
    );
    // Sources rejected before reading still need a watch projection.
    for path in &sources.files {
        hashes
            .entry(path.clone())
            .or_insert_with(|| file_hash(path));
    }
    // Newly created unresolved imports must still invalidate discovery.
    for path in &sources.unresolved_files {
        hashes.entry(path.clone()).or_insert(None);
    }
    hashes
}

fn content_hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn file_hash(path: &Path) -> Option<String> {
    super::digest_file(path).map(format_digest)
}

fn format_digest(digest: [u8; 32]) -> String {
    format!("{:x}", sha2::digest::Output::<Sha256>::from(digest))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn fixture() -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "onda_run_source_graph_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn size_rejected_project_files_remain_watchable_without_claiming_a_hash() {
        let root = fs::canonicalize(fixture()).unwrap();
        let entry = root.join("main.onda");
        let manifest = root.join("project.ondaproject");
        let asset = root.join("clip.ondabuffer");
        fs::write(&entry, "sample:\n  out1 = 0.0\n").unwrap();
        fs::write(
            &manifest,
            r#"{"entry":"main.onda","buffers":{"clip":{"file":"clip.ondabuffer"}}}"#,
        )
        .unwrap();
        fs::write(&asset, [0; 4096]).unwrap();
        let mut digests = FileReadDigests::new();
        assert!(onda_project::resolve_project_input_with_file_digests(
            &manifest,
            ProjectLimits {
                max_manifest_bytes: 1,
                ..ProjectLimits::default()
            },
            Some(&mut digests),
        )
        .is_err());
        let graph = RunSourceGraph::from_failed_sources(&SourceManifest::default(), digests);
        assert!(graph.files.contains(&manifest));
        assert!(!graph.content_hashes.contains_key(&manifest));

        let project =
            onda_project::resolve_project_input(&manifest, ProjectLimits::default()).unwrap();
        let mut digests = FileReadDigests::new();
        assert!(project
            .project()
            .unwrap()
            .load_buffer_assets_excluding(
                ProjectLimits {
                    max_asset_bytes: 1,
                    ..ProjectLimits::default()
                },
                &Default::default(),
                Some(&mut digests),
            )
            .is_err());
        let graph = RunSourceGraph::from_failed_sources(&SourceManifest::default(), digests);
        assert!(graph.files.contains(&asset));
        assert!(!graph.content_hashes.contains_key(&asset));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn source_graph_tracks_transitive_imports_and_removes_obsolete_sources() {
        let root = fixture();
        let entry = root.join("main.onda");
        let voice = root.join("voice.onda");
        let osc = root.join("osc.onda");
        let unrelated = root.join("unused.onda");
        fs::write(&entry, "import voice\n").unwrap();
        fs::write(&voice, "import osc\n").unwrap();
        fs::write(&osc, "const value = 1.0\n").unwrap();
        fs::write(&unrelated, "const value = 2.0\n").unwrap();
        let graph = run_source_graph(&entry);
        assert!(graph.complete);
        assert!(graph.files.contains(&entry));
        assert!(graph.files.contains(&voice));
        assert!(graph.files.contains(&osc));
        assert!(!graph.files.contains(&unrelated));
        fs::write(&voice, "const value = 1.0\n").unwrap();
        assert!(!run_source_graph(&entry).files.contains(&osc));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn source_graph_reports_missing_imports_and_parse_failures() {
        let root = fixture();
        let entry = root.join("main.onda");
        fs::write(&entry, "import missing\n").unwrap();
        let graph = run_source_graph(&entry);
        assert!(!graph.complete);
        assert!(graph.files.contains(&root.join("missing.onda")));
        assert_eq!(graph.content_hashes[&root.join("missing.onda")], None);
        fs::write(&entry, "this is not valid onda\n").unwrap();
        let graph = run_source_graph(&entry);
        assert!(!graph.complete);
        assert!(graph.files.contains(&entry));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn source_graph_hashes_invalid_utf8_without_losing_missing_file_validation() {
        let root = fixture();
        let entry = root.join("main.onda");
        let voice = root.join("voice.onda");
        fs::write(&entry, "import voice\n").unwrap();
        fs::write(&voice, [0xff]).unwrap();
        let graph = run_source_graph(&entry);
        assert!(!graph.complete);
        assert_eq!(graph.content_hashes[&voice], Some(content_hash(&[0xff])));

        let failed = load_program_file(&entry).unwrap_err();
        fs::write(&voice, "const value = 1.0\n").unwrap();
        assert_eq!(
            source_hashes(&failed.sources)[&voice],
            Some(content_hash(&[0xff]))
        );
        assert_ne!(source_hashes(&failed.sources)[&voice], file_hash(&voice));

        fs::remove_file(&voice).unwrap();
        let failed = load_program_file(&entry).unwrap_err();
        fs::write(&voice, "const value = 1.0\n").unwrap();
        // A newly created unresolved import must still invalidate discovery.
        assert_eq!(source_hashes(&failed.sources)[&voice], None);
        assert_ne!(source_hashes(&failed.sources)[&voice], file_hash(&voice));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn compiled_graph_replaces_discovery_with_exact_compiler_reads() {
        let root = fixture();
        let entry = root.join("main.onda");
        let voice = root.join("voice.onda");
        let child = root.join("child.onda");
        fs::write(&entry, "import voice\n").unwrap();
        fs::write(&voice, "const voice_gain = 1.0\n").unwrap();
        let discovered = run_source_graph(&entry);
        assert!(!discovered.files.contains(&child));

        let compiled_voice = "import child\nconst voice_gain = child_gain\n";
        fs::write(&voice, compiled_voice).unwrap();
        fs::write(&child, "const child_gain = 9.0\n").unwrap();
        let compiled = onda_frontend::load_program_file(&entry).unwrap();
        // A transient edit can be consumed by compilation and then reverted
        // before the host receives the graph. Hash the reads, not current disk.
        fs::write(&voice, "const voice_gain = 1.0\n").unwrap();
        let graph = RunSourceGraph::from_compiled_sources(&compiled.sources, BTreeMap::new());
        assert!(graph.complete);
        assert!(graph.files.contains(&child));
        assert_eq!(
            graph.content_hashes[&voice],
            Some(content_hash(compiled_voice.as_bytes()))
        );
        assert_ne!(
            graph.content_hashes[&voice],
            run_source_graph(&entry).content_hashes[&voice]
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn compiled_graph_uses_loaded_project_files_despite_reverted_edits() {
        let root = fixture();
        let entry = root.join("main.onda");
        let manifest = root.join("project.ondaproject");
        let asset = root.join("clip.ondabuffer");
        fs::write(&entry, "const voice_gain = 1.0\n").unwrap();
        let original_manifest = r#"{"entry":"main.onda"}"#;
        let loaded_manifest =
            r#"{"entry":"main.onda","buffers":{"clip":{"file":"clip.ondabuffer"}}}"#;
        let encode = |sample| {
            let asset = onda_project::BufferAsset::new(
                1,
                1,
                48_000.0,
                onda_project::BufferSamples::F32(vec![sample]),
            )
            .unwrap();
            onda_project::encode_ondabuffer(&asset).unwrap()
        };
        let original_asset = encode(1.0);
        let loaded_asset = encode(2.0);
        fs::write(&manifest, original_manifest).unwrap();
        fs::write(&asset, &original_asset).unwrap();
        assert!(!run_source_graph(&manifest).files.contains(&asset));

        fs::write(&manifest, loaded_manifest).unwrap();
        fs::write(&asset, &loaded_asset).unwrap();
        let input =
            onda_project::resolve_project_input(&manifest, ProjectLimits::default()).unwrap();
        let project = input.project().unwrap();
        let mut digests = BTreeMap::from([(
            project.manifest_path.clone(),
            FileReadDigest::Content(project.manifest_digest),
        )]);
        let buffers = project
            .load_buffer_assets_excluding(
                ProjectLimits::default(),
                &Default::default(),
                Some(&mut digests),
            )
            .unwrap();
        assert_eq!(
            buffers["clip"].0.samples,
            onda_project::BufferSamples::F32(vec![2.0])
        );
        let compiled = onda_frontend::load_program_file(&entry).unwrap();
        fs::write(&manifest, original_manifest).unwrap();
        fs::write(&asset, &original_asset).unwrap();
        let graph = RunSourceGraph::from_compiled_sources(&compiled.sources, digests);
        assert!(graph.complete);
        assert!(graph.files.contains(&asset));
        assert_eq!(
            graph.content_hashes[&manifest],
            Some(content_hash(loaded_manifest.as_bytes()))
        );
        assert_eq!(
            graph.content_hashes[&asset],
            Some(content_hash(&loaded_asset))
        );
        for path in [&manifest, &asset] {
            assert_ne!(graph.content_hashes[path], file_hash(path));
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn project_graph_contains_manifest_sources_and_missing_assets() {
        let root = fixture();
        let entry = root.join("main.onda");
        let manifest = root.join("project.ondaproject");
        fs::write(&entry, "import voice\n").unwrap();
        fs::write(root.join("voice.onda"), "const value = 1.0\n").unwrap();
        fs::write(
            &manifest,
            r#"{"entry":"main.onda","buffers":{"clip":{"file":"clip.wav"}}}"#,
        )
        .unwrap();
        let graph = run_source_graph(&manifest);
        assert!(graph.complete);
        for path in [
            &manifest,
            &entry,
            &root.join("voice.onda"),
            &root.join("clip.wav"),
        ] {
            assert!(graph.files.contains(path), "missing {}", path.display());
        }
        fs::write(&manifest, "{").unwrap();
        let graph = run_source_graph(&manifest);
        assert!(!graph.complete);
        assert!(graph.files.contains(&manifest));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn project_watch_snapshot_pairs_paths_and_digest_from_one_read() {
        let root = fixture();
        let manifest = root.join("project.ondaproject");
        let original = r#"{"entry":"original.onda"}"#;
        let transient = r#"{"entry":"transient.onda"}"#;
        fs::write(&manifest, original).unwrap();
        let before = resolve_project_watch_snapshot(&manifest, ProjectLimits::default());
        fs::write(&manifest, transient).unwrap();
        let during = resolve_project_watch_snapshot(&manifest, ProjectLimits::default());
        fs::write(&manifest, original).unwrap();
        assert_eq!(before.paths.unwrap().entry, root.join("original.onda"));
        assert_eq!(during.paths.unwrap().entry, root.join("transient.onda"));
        assert_eq!(
            during.manifest_digest.content_digest().map(format_digest),
            Some(content_hash(transient.as_bytes()))
        );
        assert_ne!(during.manifest_digest, before.manifest_digest);

        // Invalid manifests need their consumed fingerprint too: a parse error
        // must not become associated with a later valid revision on disk.
        fs::write(&manifest, "{").unwrap();
        let invalid = resolve_project_watch_snapshot(&manifest, ProjectLimits::default());
        fs::write(&manifest, original).unwrap();
        assert!(invalid.paths.is_err());
        assert_eq!(
            invalid.manifest_digest.content_digest().map(format_digest),
            Some(content_hash(b"{"))
        );
        assert_ne!(
            invalid.manifest_digest.content_digest().map(format_digest),
            file_hash(&manifest)
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn failed_build_graph_preserves_transient_reads_after_disk_is_restored() {
        let root = fixture();
        let entry = root.join("main.onda");
        let original = "sample:\n  out1 = 1.0\n";
        let transient = "sample:\n  out1 = missing_name\n";
        fs::write(&entry, original).unwrap();
        let preliminary = run_source_graph(&entry);
        fs::write(&entry, transient).unwrap();
        let error = onda_daemon::RunSession::build(
            &onda_daemon::AnalysisSession::new(),
            &entry,
            onda_daemon::RunOptions::default(),
        )
        .unwrap_err();
        fs::write(&entry, original).unwrap();
        let graph = RunSourceGraph::from_failed_sources(&error.sources, BTreeMap::new());
        let mut wire = Vec::new();
        graph.write_event(&mut wire).unwrap();
        let event: serde_json::Value = serde_json::from_slice(&wire).unwrap();
        assert_eq!(event["event"], "sources");
        assert_eq!(event["complete"], false);
        assert_eq!(
            graph.content_hashes[&entry],
            Some(content_hash(transient.as_bytes()))
        );
        assert_eq!(preliminary.content_hashes[&entry], file_hash(&entry));
        assert_ne!(graph.content_hashes[&entry], file_hash(&entry));
        fs::remove_dir_all(root).unwrap();
    }
}
