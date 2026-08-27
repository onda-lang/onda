use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use onda_project::{
    decode_buffer_bytes, decode_ondabuffer, encode_ondabuffer, encode_wav_f32, inspect_buffer_file,
    load_buffer_file, resolve_project_input, resolve_project_watch_paths,
    validate_buffer_asset_metadata, validate_buffer_assets, AssetId, BufferAsset, BufferElement,
    BufferSamples, InlineBuffer, MaterializationPlan, PlannedFile, ProjectBufferChannels,
    ProjectBufferDeclaration, ProjectConstValue, ProjectImage, ProjectInput, ProjectLimits,
    ProjectManifest, SourceDocument, SourceImage, SourceReferenceKind, SourceResolution,
    ONDA_PROJECT_DEFAULT_FILE_NAME,
};
use serde_json::json;

fn sample_assets() -> Vec<BufferAsset> {
    vec![
        BufferAsset::new(2, 2, 48_000.0, BufferSamples::Bool(vec![0, 1, 1, 0]))
            .expect("valid bool asset"),
        BufferAsset::new(2, 2, 48_000.0, BufferSamples::I32(vec![-1, 2, 3, i32::MAX]))
            .expect("valid i32 asset"),
        BufferAsset::new(
            2,
            2,
            48_000.0,
            BufferSamples::I64(vec![i64::MIN, -1, 0, i64::MAX]),
        )
        .expect("valid i64 asset"),
        BufferAsset::new(
            2,
            2,
            48_000.0,
            BufferSamples::F32(vec![-0.0, 1.25, f32::INFINITY, -2.5]),
        )
        .expect("valid f32 asset"),
        BufferAsset::new(
            2,
            2,
            48_000.0,
            BufferSamples::F64(vec![-0.0, 1.25, f64::INFINITY, -2.5]),
        )
        .expect("valid f64 asset"),
    ]
}

#[test]
fn ondabuffer_round_trips_every_primitive_type_exactly() {
    for asset in sample_assets() {
        let encoded = encode_ondabuffer(&asset).expect("encode Onda buffer");
        let decoded =
            decode_ondabuffer(&encoded, ProjectLimits::default()).expect("decode Onda buffer");
        assert_eq!(decoded, asset);
    }
}

#[test]
fn ondabuffer_streams_numeric_payloads_across_internal_chunks() {
    let samples = (0..10_000)
        .map(|index| i64::from(index) * -17)
        .collect::<Vec<_>>();
    let asset = BufferAsset::new(
        samples.len() as u32,
        1,
        48_000.0,
        BufferSamples::I64(samples),
    )
    .expect("valid multi-chunk asset");

    let encoded = encode_ondabuffer(&asset).expect("encode multi-chunk asset");
    let decoded =
        decode_ondabuffer(&encoded, ProjectLimits::default()).expect("decode multi-chunk asset");
    assert_eq!(decoded, asset);
}

#[test]
fn ondabuffer_rejects_content_corruption() {
    let asset =
        BufferAsset::new(2, 1, 48_000.0, BufferSamples::I32(vec![1, 2])).expect("valid asset");
    let mut encoded = encode_ondabuffer(&asset).expect("encode Onda buffer");
    let last = encoded.len() - 1;
    encoded[last] ^= 1;
    let error = decode_ondabuffer(&encoded, ProjectLimits::default())
        .expect_err("corrupt payload must fail");
    assert!(error.to_string().contains("digest mismatch"));
}

#[test]
fn buffer_file_inspection_reads_shape_without_validating_payload_content() {
    let directory = temporary_directory("buffer-metadata");
    fs::create_dir_all(&directory).expect("create metadata test directory");
    let asset = BufferAsset::new(2, 2, 44_100.0, BufferSamples::I32(vec![1, 2, 3, 4]))
        .expect("valid metadata fixture");
    let mut encoded = encode_ondabuffer(&asset).expect("encode metadata fixture");
    let last = encoded.len() - 1;
    encoded[last] ^= 1;
    let path = directory.join("corrupt-payload.ondabuffer");
    fs::write(&path, encoded).expect("write metadata fixture");

    assert_eq!(
        inspect_buffer_file(&path, ProjectLimits::default()).expect("inspect header"),
        asset.metadata()
    );
    load_buffer_file(&path, ProjectLimits::default())
        .expect_err("runtime loading must still validate the content digest");

    fs::remove_dir_all(directory).expect("remove metadata test directory");
}

#[test]
fn wav_file_inspection_reports_decoded_f32_shape() {
    let directory = temporary_directory("wav-metadata");
    fs::create_dir_all(&directory).expect("create WAV metadata test directory");
    let asset = BufferAsset::new(
        3,
        2,
        48_000.0,
        BufferSamples::F32(vec![0.0, 0.1, 0.2, 0.3, 0.4, 0.5]),
    )
    .expect("valid WAV metadata fixture");
    let path = directory.join("sample.wav");
    fs::write(
        &path,
        encode_wav_f32(&asset).expect("encode WAV metadata fixture"),
    )
    .expect("write WAV metadata fixture");

    let metadata =
        inspect_buffer_file(&path, ProjectLimits::default()).expect("inspect WAV metadata");
    assert_eq!(metadata, asset.metadata());
    validate_buffer_asset_metadata(
        [("sample", &metadata)],
        &[ProjectBufferDeclaration {
            name: "sample".to_owned(),
            element: BufferElement::F32,
            channels: ProjectBufferChannels::Static(2),
            array_len: 1,
            is_array: false,
        }],
    )
    .expect("metadata matches declaration");

    fs::remove_dir_all(directory).expect("remove WAV metadata test directory");
}

#[test]
fn inline_buffers_use_exact_type_rules() {
    let i64_asset = InlineBuffer {
        element: BufferElement::I64,
        channels: 2,
        sample_rate: 1.0,
        values: vec![json!("-9223372036854775808"), json!("9223372036854775807")],
    }
    .to_asset(&ProjectLimits::default())
    .expect("decimal i64 strings");
    assert_eq!(
        i64_asset.samples,
        BufferSamples::I64(vec![i64::MIN, i64::MAX])
    );

    let invalid = InlineBuffer {
        element: BufferElement::I64,
        channels: 1,
        sample_rate: 1.0,
        values: vec![json!(42)],
    };
    assert!(invalid.to_asset(&ProjectLimits::default()).is_err());
}

#[test]
fn project_image_round_trips_and_materializes() {
    let asset = BufferAsset::new(2, 1, 48_000.0, BufferSamples::F32(vec![0.25, -0.5]))
        .expect("valid asset");
    let asset_id = AssetId::for_buffer(&asset);
    let sources = SourceImage {
        entry: "src/patch.onda".to_owned(),
        stdlib_digest: onda_project::current_stdlib_digest(),
        documents: vec![
            SourceDocument {
                path: "src/patch.onda".to_owned(),
                contents: "import dsp/gain\n".to_owned(),
            },
            SourceDocument {
                path: "src/dsp/gain.onda".to_owned(),
                contents: "const amount = 0.5\n".to_owned(),
            },
        ],
        resolutions: vec![SourceResolution {
            source: "src/patch.onda".to_owned(),
            kind: SourceReferenceKind::Import,
            specifier: "dsp/gain".to_owned(),
            target: "src/dsp/gain.onda".to_owned(),
        }],
    };
    let image = ProjectImage::new(
        sources,
        BTreeMap::from([("sample".to_owned(), asset_id.clone())]),
        BTreeMap::from([(asset_id, asset)]),
    )
    .expect("valid image");
    let encoded = image.serialize().expect("serialize image");
    let decoded =
        ProjectImage::deserialize(&encoded, ProjectLimits::default()).expect("deserialize image");
    assert_eq!(decoded, image);
    decoded
        .sources()
        .replay(ProjectLimits::default())
        .expect("replay exact source graph");

    let plan = decoded.materialization_plan().expect("materialize image");
    assert!(plan
        .files
        .iter()
        .any(|file| file.relative_path == ONDA_PROJECT_DEFAULT_FILE_NAME));
    assert!(plan
        .files
        .iter()
        .any(|file| file.relative_path == "assets/sample.ondabuffer"));
    assert!(plan
        .files
        .iter()
        .any(|file| file.relative_path == "code/main.onda"));
    assert!(plan
        .files
        .iter()
        .any(|file| file.relative_path == "code/src/dsp/gain.onda"));
    let manifest = plan
        .files
        .iter()
        .find(|file| file.relative_path == ONDA_PROJECT_DEFAULT_FILE_NAME)
        .expect("materialized manifest");
    assert_eq!(
        serde_json::from_slice::<ProjectManifest>(&manifest.bytes)
            .expect("parse materialized manifest")
            .entry,
        "code/main.onda"
    );

    let named_plan = decoded
        .materialization_plan_with_asset_file_names(&BTreeMap::from([(
            "sample".to_owned(),
            "Acoustic Snare.wav".to_owned(),
        )]))
        .expect("materialize image with original asset names");
    assert!(named_plan
        .files
        .iter()
        .any(|file| file.relative_path == "assets/Acoustic Snare.ondabuffer"));

    let explicitly_named_plan = decoded
        .materialization_plan_with_file_names("drums.ondaproject", &BTreeMap::new())
        .expect("materialize image with explicit project filename");
    assert!(explicitly_named_plan
        .files
        .iter()
        .any(|file| file.relative_path == "drums.ondaproject"));
    for invalid_name in ["project.json", "nested/project.ondaproject"] {
        decoded
            .materialization_plan_with_file_names(invalid_name, &BTreeMap::new())
            .expect_err("exported project filename must be a .ondaproject basename");
    }
}

#[test]
fn canonical_materialization_rewrites_entry_collisions() {
    let sources = SourceImage {
        entry: "patch.onda".to_owned(),
        stdlib_digest: onda_project::current_stdlib_digest(),
        documents: vec![
            SourceDocument {
                path: "patch.onda".to_owned(),
                contents: "include \"main.onda\"\nouts 1\nsample:\n  out1 = LEVEL\n".to_owned(),
            },
            SourceDocument {
                path: "main.onda".to_owned(),
                contents: "const LEVEL = 0.25\n".to_owned(),
            },
        ],
        resolutions: vec![SourceResolution {
            source: "patch.onda".to_owned(),
            kind: SourceReferenceKind::Include,
            specifier: "main.onda".to_owned(),
            target: "main.onda".to_owned(),
        }],
    };
    let image = ProjectImage::from_buffer_assets(sources, BTreeMap::new()).expect("valid image");
    let plan = image
        .materialization_plan()
        .expect("canonical materialization");
    let files = plan
        .files
        .into_iter()
        .map(|file| (file.relative_path, file.bytes))
        .collect::<BTreeMap<_, _>>();

    assert!(files.contains_key("code/main.onda"));
    assert!(files.contains_key("code/main-2.onda"));
    assert!(std::str::from_utf8(&files["code/main.onda"])
        .expect("UTF-8 entry")
        .starts_with("include \"main-2.onda\""));
    ProjectImage::from_materialized_files(&files, ProjectLimits::default())
        .expect("canonical collision output reloads")
        .sources()
        .replay(ProjectLimits::default())
        .expect("canonical collision output replays");
}

#[test]
fn canonical_materialization_is_idempotent_after_reload() {
    let sources = SourceImage {
        entry: "main.onda".to_owned(),
        stdlib_digest: onda_project::current_stdlib_digest(),
        documents: vec![
            SourceDocument {
                path: "main.onda".to_owned(),
                contents: "import dsp/gain\n".to_owned(),
            },
            SourceDocument {
                path: "dsp/gain.onda".to_owned(),
                contents: "const GAIN = 0.5\n".to_owned(),
            },
        ],
        resolutions: vec![SourceResolution {
            source: "main.onda".to_owned(),
            kind: SourceReferenceKind::Import,
            specifier: "dsp/gain".to_owned(),
            target: "dsp/gain.onda".to_owned(),
        }],
    };
    let first = ProjectImage::from_buffer_assets(sources, BTreeMap::new())
        .expect("valid project image")
        .materialization_plan()
        .expect("first canonical materialization");
    let files = first
        .files
        .iter()
        .map(|file| (file.relative_path.clone(), file.bytes.clone()))
        .collect::<BTreeMap<_, _>>();
    let second = ProjectImage::from_materialized_files(&files, ProjectLimits::default())
        .expect("reload canonical project")
        .materialization_plan()
        .expect("second canonical materialization");

    assert_eq!(second, first);
}

#[test]
fn project_image_serialization_enforces_the_encoded_manifest_limit() {
    let image = ProjectImage::from_buffer_assets(
        SourceImage {
            entry: "main.onda".to_owned(),
            stdlib_digest: onda_project::current_stdlib_digest(),
            documents: vec![SourceDocument {
                path: "main.onda".to_owned(),
                contents: "outs 1\nsample:\n  out1 = 0.0\n".to_owned(),
            }],
            resolutions: Vec::new(),
        },
        BTreeMap::new(),
    )
    .expect("valid image");
    let limits = ProjectLimits {
        max_manifest_bytes: 1,
        ..ProjectLimits::default()
    };

    let error = image
        .serialize_with_limits(limits)
        .expect_err("encoded project manifest must honor its byte limit");
    assert!(error.to_string().contains("project image manifest"));
}

#[test]
fn materialization_plans_reject_paths_outside_the_project() {
    for relative_path in ["../escape", "/absolute", "src/../../escape"] {
        let plan = MaterializationPlan {
            directories: Vec::new(),
            files: vec![PlannedFile {
                relative_path: relative_path.to_owned(),
                bytes: Vec::new(),
            }],
        };
        plan.validate(&ProjectLimits::default())
            .expect_err("publication paths must remain below the project root");
    }
}

#[test]
fn portable_paths_enforce_the_component_byte_limit() {
    let limits = ProjectLimits::default();
    let plan = |relative_path: String| MaterializationPlan {
        directories: Vec::new(),
        files: vec![PlannedFile {
            relative_path,
            bytes: Vec::new(),
        }],
    };

    plan("a".repeat(limits.max_path_component_bytes))
        .validate(&limits)
        .expect("a component at the portable byte limit is valid");
    plan("a".repeat(limits.max_path_component_bytes + 1))
        .validate(&limits)
        .expect_err("an oversized ASCII component is not portable");
    plan("é".repeat(limits.max_path_component_bytes.div_ceil(2)))
        .validate(&limits)
        .expect_err("the component limit is measured in UTF-8 bytes");
}

#[test]
fn portable_paths_reject_every_windows_device_name_spelling() {
    let limits = ProjectLimits::default();
    for relative_path in [
        "COM¹.onda",
        "com².on",
        "LPT³.ondabuffer",
        "CONIN$.onda",
        "conout$.ondabuffer",
    ] {
        let plan = MaterializationPlan {
            directories: Vec::new(),
            files: vec![PlannedFile {
                relative_path: relative_path.to_owned(),
                bytes: Vec::new(),
            }],
        };
        plan.validate(&limits)
            .expect_err("Windows device names are not portable project paths");
    }
}

#[test]
fn materialized_asset_names_are_portable_and_collision_safe() {
    let sources = SourceImage {
        entry: "main.onda".to_owned(),
        stdlib_digest: onda_project::current_stdlib_digest(),
        documents: vec![SourceDocument {
            path: "main.onda".to_owned(),
            contents: String::new(),
        }],
        resolutions: Vec::new(),
    };
    let first =
        BufferAsset::new(1, 1, 48_000.0, BufferSamples::F32(vec![0.0])).expect("valid first asset");
    let second = BufferAsset::new(1, 1, 48_000.0, BufferSamples::F32(vec![1.0]))
        .expect("valid second asset");
    let image = ProjectImage::from_buffer_assets(
        sources,
        BTreeMap::from([("first".to_owned(), first), ("second".to_owned(), second)]),
    )
    .expect("valid image");
    let plan = image
        .materialization_plan_with_asset_file_names(&BTreeMap::from([
            ("first".to_owned(), "CON.wav".to_owned()),
            ("second".to_owned(), "con.wav".to_owned()),
        ]))
        .expect("materialize colliding original filenames");
    let asset_paths = plan
        .files
        .iter()
        .map(|file| file.relative_path.to_ascii_lowercase())
        .filter(|path| path.ends_with(".ondabuffer"))
        .collect::<Vec<_>>();
    assert_eq!(asset_paths.len(), 2);
    assert!(asset_paths.contains(&"assets/_con.ondabuffer".to_owned()));
    assert!(asset_paths
        .iter()
        .any(|path| path.starts_with("assets/_con-") && path.ends_with(".ondabuffer")));
}

#[test]
fn project_buffer_bindings_validate_against_compiled_declarations() {
    let asset = BufferAsset::new(2, 2, 48_000.0, BufferSamples::I64(vec![1, 2, 3, 4]))
        .expect("valid asset");
    let sources = SourceImage {
        entry: "main.onda".to_owned(),
        stdlib_digest: onda_project::current_stdlib_digest(),
        documents: vec![SourceDocument {
            path: "main.onda".to_owned(),
            contents: "buffers:\n  sequence: buffer<i64[2]>\n".to_owned(),
        }],
        resolutions: Vec::new(),
    };
    let image =
        ProjectImage::from_buffer_assets(sources, BTreeMap::from([("sequence".to_owned(), asset)]))
            .expect("structurally valid image");
    image
        .validate_buffer_declarations(&[ProjectBufferDeclaration {
            name: "sequence".to_owned(),
            element: BufferElement::I64,
            channels: ProjectBufferChannels::Static(2),
            array_len: 1,
            is_array: false,
        }])
        .expect("matching declaration");
    let error = image
        .validate_buffer_declarations(&[ProjectBufferDeclaration {
            name: "sequence".to_owned(),
            element: BufferElement::F32,
            channels: ProjectBufferChannels::Static(2),
            array_len: 1,
            is_array: false,
        }])
        .expect_err("mismatched element type");
    assert!(error.to_string().contains("requires f32"));
}

#[test]
fn canonical_float_wav_is_readable_as_the_same_f32_asset() {
    let asset = BufferAsset::new(
        2,
        2,
        48_000.0,
        BufferSamples::F32(vec![0.25, -0.5, 0.75, -1.0]),
    )
    .expect("valid asset");
    let directory = temporary_directory("wav");
    fs::create_dir_all(&directory).expect("create test directory");
    let path = directory.join("asset.wav");
    fs::write(&path, encode_wav_f32(&asset).expect("encode WAV")).expect("write WAV");
    let decoded = load_buffer_file(&path, ProjectLimits::default()).expect("read WAV");
    assert_eq!(decoded, asset);
    fs::remove_dir_all(directory).expect("remove test directory");
}

#[test]
fn integer_wav_uses_symmetric_full_scale_normalization() {
    let wav = pcm16_wav(&[i16::MIN, i16::MAX]);
    let decoded = decode_buffer_bytes(&wav, "asset.wav", ProjectLimits::default())
        .expect("decode integer WAV");
    assert_eq!(
        decoded.samples,
        BufferSamples::F32(vec![-1.0, i16::MAX as f32 / 32_768.0])
    );
}

#[test]
fn wav_limits_are_checked_before_decoding_samples() {
    let wav = pcm16_wav(&[1, 2]);
    let limits = ProjectLimits {
        max_asset_bytes: 4,
        ..ProjectLimits::default()
    };
    let error = decode_buffer_bytes(&wav, "asset.wav", limits)
        .expect_err("decoded f32 payload exceeds limit");
    assert!(error.to_string().contains("decodes to 8 bytes"));

    let directory = temporary_directory("oversized-wav");
    fs::create_dir_all(&directory).expect("create test directory");
    let path = directory.join("asset.wav");
    fs::write(&path, pcm16_wav(&[0; 16])).expect("write WAV");
    let error = load_buffer_file(
        &path,
        ProjectLimits {
            max_asset_bytes: 1,
            ..ProjectLimits::default()
        },
    )
    .expect_err("encoded file exceeds pre-read limit");
    assert!(error.to_string().contains("encoded-file limit"));
    fs::remove_dir_all(directory).expect("remove test directory");
}

#[test]
fn source_images_reject_nonportable_file_collisions() {
    for paths in [
        vec!["Main.onda", "main.onda"],
        vec!["\u{e9}.onda", "e\u{301}.onda"],
        vec!["σ.onda", "ς.onda"],
        vec!["ẞ.onda", "ss.onda"],
        vec!["main.onda", "main.onda/child.onda"],
        vec!["project.ondaproject/main.onda"],
    ] {
        let entry = paths[0].to_owned();
        let sources = SourceImage {
            entry,
            stdlib_digest: onda_project::current_stdlib_digest(),
            documents: paths
                .into_iter()
                .map(|path| SourceDocument {
                    path: path.to_owned(),
                    contents: String::new(),
                })
                .collect(),
            resolutions: Vec::new(),
        };
        ProjectImage::from_buffer_assets(sources, BTreeMap::new())
            .expect_err("nonportable source paths must fail");
    }

    let asset =
        BufferAsset::new(1, 1, 48_000.0, BufferSamples::F32(vec![0.0])).expect("valid asset");
    let path = "Assets/clip.ondabuffer".to_owned();
    let sources = SourceImage {
        entry: path.clone(),
        stdlib_digest: onda_project::current_stdlib_digest(),
        documents: vec![SourceDocument {
            path,
            contents: String::new(),
        }],
        resolutions: Vec::new(),
    };
    let plan =
        ProjectImage::from_buffer_assets(sources, BTreeMap::from([("clip".to_owned(), asset)]))
            .expect("presentation filenames do not affect image validity")
            .materialization_plan()
            .expect("materialization resolves source and asset filename collisions");
    assert!(plan
        .files
        .iter()
        .any(|file| file.relative_path == "assets/clip.ondabuffer"));
}

#[test]
fn project_images_apply_manifest_buffer_name_constraints() {
    let asset =
        BufferAsset::new(1, 1, 48_000.0, BufferSamples::F32(vec![0.0])).expect("valid asset");
    let sources = SourceImage {
        entry: "main.onda".to_owned(),
        stdlib_digest: onda_project::current_stdlib_digest(),
        documents: vec![SourceDocument {
            path: "main.onda".to_owned(),
            contents: String::new(),
        }],
        resolutions: Vec::new(),
    };

    for name in ["contains\0nul".to_owned(), "x".repeat(16 * 1024 + 1)] {
        ProjectImage::from_buffer_assets(sources.clone(), BTreeMap::from([(name, asset.clone())]))
            .expect_err("image buffer names must be valid in every project transport");
    }

    let mut invalid_resolution_sources = sources;
    invalid_resolution_sources.resolutions = vec![SourceResolution {
        source: "main.onda".to_owned(),
        kind: SourceReferenceKind::Include,
        specifier: "contains\0nul".to_owned(),
        target: "main.onda".to_owned(),
    }];
    ProjectImage::from_buffer_assets(invalid_resolution_sources, BTreeMap::new())
        .expect_err("image resolution specifiers must be valid in the C transport");
}

#[test]
fn project_images_reject_ambiguous_buffer_slot_bindings() {
    let asset =
        BufferAsset::new(1, 1, 48_000.0, BufferSamples::F32(vec![0.0])).expect("valid asset");
    let sources = SourceImage {
        entry: "main.onda".to_owned(),
        stdlib_digest: onda_project::current_stdlib_digest(),
        documents: vec![SourceDocument {
            path: "main.onda".to_owned(),
            contents: "buffers:\n  bank: f32 {1}\n".to_owned(),
        }],
        resolutions: Vec::new(),
    };

    let error = ProjectImage::from_buffer_assets(
        sources.clone(),
        BTreeMap::from([("bank[00]".to_owned(), asset.clone())]),
    )
    .expect_err("array slot indices must use one canonical spelling");
    assert!(error.to_string().contains("canonical '[0]' notation"));

    let error = ProjectImage::from_buffer_assets(
        sources.clone(),
        BTreeMap::from([("bank[4096]".to_owned(), asset.clone())]),
    )
    .expect_err("sparse array slots must remain within the materialization budget");
    assert!(error.to_string().contains("4096 slot limit"));

    let error = ProjectImage::from_buffer_assets(
        sources,
        BTreeMap::from([
            ("bank".to_owned(), asset.clone()),
            ("bank[0]".to_owned(), asset),
        ]),
    )
    .expect_err("scalar and array-slot bindings must not materialize over each other");
    assert!(error.to_string().contains("both scalar and array-slot"));
}

#[test]
fn materialized_projects_preserve_unreferenced_editable_sources() {
    let manifest = ProjectManifest::empty("main.onda")
        .to_pretty_json()
        .expect("manifest JSON");
    let files = BTreeMap::from([
        ("session.ondaproject".to_owned(), manifest.into_bytes()),
        (
            "main.onda".to_owned(),
            b"outs 1\nsample:\n  out1 = 0.0\n".to_vec(),
        ),
        (
            "scratch.onda".to_owned(),
            b"incomplete work in progress\n".to_vec(),
        ),
    ]);

    let image = ProjectImage::from_materialized_files(&files, ProjectLimits::default())
        .expect("load editable project files");
    assert_eq!(
        image
            .sources()
            .documents
            .iter()
            .map(|document| document.path.as_str())
            .collect::<Vec<_>>(),
        vec!["main.onda", "scratch.onda"]
    );
    let materialized = image.materialization_plan().expect("materialize project");
    assert!(materialized
        .files
        .iter()
        .any(|file| file.relative_path == "code/scratch.onda"));
}

#[test]
fn materialized_projects_load_an_entry_regardless_of_extension() {
    let manifest = ProjectManifest::empty("main")
        .to_pretty_json()
        .expect("manifest JSON");
    let files = BTreeMap::from([
        ("session.ondaproject".to_owned(), manifest.into_bytes()),
        (
            "main".to_owned(),
            b"include \"shared.on\"\nouts 1\nsample:\n  out1 = LEVEL\n".to_vec(),
        ),
        ("shared.on".to_owned(), b"const LEVEL = 0.25\n".to_vec()),
    ]);

    let image = ProjectImage::from_materialized_files(&files, ProjectLimits::default())
        .expect("load project with an extensionless entry");
    assert!(image
        .sources()
        .documents
        .iter()
        .any(|document| document.path == "main"));
    image
        .sources()
        .replay(ProjectLimits::default())
        .expect("replay source graph with an extensionless entry");
}

#[test]
fn materialized_projects_classify_root_project_extensions_by_role() {
    let manifest = ProjectManifest::empty("main.ondaproject")
        .to_pretty_json()
        .expect("manifest JSON");
    let files = BTreeMap::from([
        ("session.ondaproject".to_owned(), manifest.into_bytes()),
        (
            "main.ondaproject".to_owned(),
            b"outs 1\nsample:\n  out1 = 0.0\n".to_vec(),
        ),
    ]);

    let image = ProjectImage::from_materialized_files(&files, ProjectLimits::default())
        .expect("the manifest entry must take precedence over its filename extension");
    assert_eq!(image.sources().entry, "main.ondaproject");
}

#[test]
fn materialized_project_roles_take_precedence_over_file_extensions() {
    let asset =
        BufferAsset::new(1, 1, 48_000.0, BufferSamples::I32(vec![42])).expect("valid asset");
    let mut manifest = ProjectManifest::empty("main.onda");
    manifest.buffers.insert(
        "sample".to_owned(),
        serde_json::from_value(json!({ "file": "sample.ondaproject" })).expect("file binding"),
    );
    let files = BTreeMap::from([
        (
            "session.ondaproject".to_owned(),
            manifest
                .to_pretty_json()
                .expect("manifest JSON")
                .into_bytes(),
        ),
        (
            "main.onda".to_owned(),
            b"buffers:\n  sample: i32\n".to_vec(),
        ),
        (
            "sample.ondaproject".to_owned(),
            encode_ondabuffer(&asset).expect("encode asset"),
        ),
    ]);

    let image = ProjectImage::from_materialized_files(&files, ProjectLimits::default())
        .expect("load buffer whose extension resembles project data");
    assert_eq!(image.assets().values().next(), Some(&asset));
}

#[test]
fn materialization_separates_sources_from_the_canonical_asset_directory() {
    let asset =
        BufferAsset::new(1, 1, 48_000.0, BufferSamples::F32(vec![0.25])).expect("valid asset");
    let image = ProjectImage::from_buffer_assets(
        SourceImage {
            entry: "assets".to_owned(),
            stdlib_digest: onda_project::current_stdlib_digest(),
            documents: vec![SourceDocument {
                path: "assets".to_owned(),
                contents: "buffers:\n  sample: f32\n".to_owned(),
            }],
            resolutions: Vec::new(),
        },
        BTreeMap::from([("sample".to_owned(), asset)]),
    )
    .expect("valid image with a source named assets");

    let plan = image
        .materialization_plan()
        .expect("materialize through a collision-free asset directory");
    assert_eq!(plan.directories, vec!["assets", "code"]);
    assert!(plan
        .files
        .iter()
        .any(|file| file.relative_path == "assets/sample.ondabuffer"));

    let files = plan
        .files
        .into_iter()
        .map(|file| (file.relative_path, file.bytes))
        .collect();
    let reloaded = ProjectImage::from_materialized_files(&files, ProjectLimits::default())
        .expect("reload materialized project with a source named assets");
    assert_eq!(reloaded.sources().entry, "code/main.onda");
}

#[test]
fn unselected_materialized_projects_require_one_unambiguous_manifest() {
    let manifest = ProjectManifest::empty("main.onda")
        .to_pretty_json()
        .expect("manifest JSON")
        .into_bytes();
    let source = b"outs 1\nsample:\n  out1 = 0.0\n".to_vec();

    let missing = BTreeMap::from([("main.onda".to_owned(), source.clone())]);
    ProjectImage::from_materialized_files(&missing, ProjectLimits::default())
        .expect_err("a materialized project needs an .ondaproject file");

    let duplicate = BTreeMap::from([
        ("first.ondaproject".to_owned(), manifest.clone()),
        ("second.ondaproject".to_owned(), manifest),
        ("main.onda".to_owned(), source.clone()),
    ]);
    ProjectImage::from_materialized_files(&duplicate, ProjectLimits::default())
        .expect_err("a materialized project cannot have ambiguous entry files");

    let nested = BTreeMap::from([
        (
            "nested/project.ondaproject".to_owned(),
            ProjectManifest::empty("main.onda")
                .to_pretty_json()
                .expect("manifest JSON")
                .into_bytes(),
        ),
        ("nested/main.onda".to_owned(), source),
    ]);
    let nested = ProjectImage::from_materialized_files(&nested, ProjectLimits::default())
        .expect("a project manifest may live in any archive directory");
    assert_eq!(nested.sources().entry, "nested/main.onda");
}

#[test]
fn materialized_projects_can_select_one_manifest_from_a_larger_file_set() {
    let first_manifest = ProjectManifest::empty("first.onda")
        .to_pretty_json()
        .expect("first manifest JSON")
        .into_bytes();
    let second_manifest = ProjectManifest::empty("second.onda")
        .to_pretty_json()
        .expect("second manifest JSON")
        .into_bytes();
    let files = BTreeMap::from([
        ("first.ondaproject".to_owned(), first_manifest),
        ("second.ondaproject".to_owned(), second_manifest),
        (
            "first.onda".to_owned(),
            b"outs 1\nsample:\n  out1 = 1.0\n".to_vec(),
        ),
        (
            "second.onda".to_owned(),
            b"outs 1\nsample:\n  out1 = 2.0\n".to_vec(),
        ),
    ]);

    ProjectImage::from_materialized_files(&files, ProjectLimits::default())
        .expect_err("an unselected multi-project file set is ambiguous");
    let selected = ProjectImage::from_materialized_files_with_manifest(
        &files,
        "second.ondaproject",
        ProjectLimits::default(),
    )
    .expect("explicit project selection resolves the file set");
    assert_eq!(selected.sources().entry, "second.onda");

    ProjectImage::from_materialized_files_with_manifest(
        &files,
        "missing.ondaproject",
        ProjectLimits::default(),
    )
    .expect_err("the selected manifest must exist");
}

#[test]
fn nested_materialized_manifests_resolve_their_own_assets() {
    let asset =
        BufferAsset::new(1, 1, 48_000.0, BufferSamples::I32(vec![42])).expect("valid asset");
    let mut manifest = ProjectManifest::empty("source/entry.on");
    manifest.buffers.insert(
        "sample".to_owned(),
        serde_json::from_value(json!({ "file": "media/sample.ondabuffer" })).expect("file binding"),
    );
    let files = BTreeMap::from([
        (
            "sessions/demo/demo.ondaproject".to_owned(),
            manifest
                .to_pretty_json()
                .expect("manifest JSON")
                .into_bytes(),
        ),
        (
            "sessions/demo/source/entry.on".to_owned(),
            b"buffers:\n  sample: i32\nouts 1\nsample:\n  out1 = 0.0\n".to_vec(),
        ),
        (
            "sessions/demo/media/sample.ondabuffer".to_owned(),
            encode_ondabuffer(&asset).expect("encode asset"),
        ),
    ]);

    let image = ProjectImage::from_materialized_files(&files, ProjectLimits::default())
        .expect("load nested project");
    assert_eq!(image.sources().entry, "sessions/demo/source/entry.on");
    assert_eq!(image.assets().values().next(), Some(&asset));
}

#[test]
fn materialized_workspace_projects_share_sources() {
    let first_manifest = ProjectManifest::empty("first.onda")
        .to_pretty_json()
        .expect("first manifest JSON")
        .into_bytes();
    let second_manifest = ProjectManifest::empty("second.onda")
        .to_pretty_json()
        .expect("second manifest JSON")
        .into_bytes();
    let files = BTreeMap::from([
        ("first.ondaproject".to_owned(), first_manifest),
        ("second.ondaproject".to_owned(), second_manifest),
        (
            "first.onda".to_owned(),
            b"include \"shared/constants.onda\"\nouts 1\nsample:\n  out1 = LEVEL\n".to_vec(),
        ),
        (
            "second.onda".to_owned(),
            b"include \"shared/constants.onda\"\nouts 1\nsample:\n  out1 = LEVEL * 2.0\n".to_vec(),
        ),
        (
            "shared/constants.onda".to_owned(),
            b"const LEVEL = 0.25\n".to_vec(),
        ),
    ]);

    let image = ProjectImage::from_materialized_files_with_manifest(
        &files,
        "first.ondaproject",
        ProjectLimits::default(),
    )
    .expect("selected project loads from the shared source workspace");
    assert_eq!(image.sources().entry, "first.onda");
    assert_eq!(
        image
            .sources()
            .documents
            .iter()
            .map(|document| document.path.as_str())
            .collect::<Vec<_>>(),
        vec!["first.onda", "second.onda", "shared/constants.onda"]
    );
    image
        .sources()
        .replay(ProjectLimits::default())
        .expect("the selected project's reachable graph replays exactly");
}

#[test]
fn every_root_manifest_participates_in_workspace_file_roles() {
    let selected_manifest = ProjectManifest::empty("selected.onda")
        .to_pretty_json()
        .expect("selected manifest JSON")
        .into_bytes();
    let mut other_manifest = ProjectManifest::empty("other.onda");
    other_manifest.buffers.insert(
        "data".to_owned(),
        serde_json::from_value(json!({ "file": "assets/data.onda" }))
            .expect("other project buffer binding"),
    );
    let asset =
        BufferAsset::new(1, 1, 1.0, BufferSamples::I32(vec![42])).expect("valid typed asset");
    let files = BTreeMap::from([
        ("selected.ondaproject".to_owned(), selected_manifest),
        (
            "other.ondaproject".to_owned(),
            other_manifest
                .to_pretty_json()
                .expect("other manifest JSON")
                .into_bytes(),
        ),
        (
            "selected.onda".to_owned(),
            b"outs 1\nsample:\n  out1 = 0.0\n".to_vec(),
        ),
        ("other.onda".to_owned(), b"buffers:\n  data: i32\n".to_vec()),
        (
            "assets/data.onda".to_owned(),
            encode_ondabuffer(&asset).expect("encode typed asset"),
        ),
    ]);

    let image = ProjectImage::from_materialized_files_with_manifest(
        &files,
        "selected.ondaproject",
        ProjectLimits::default(),
    )
    .expect("an unselected project's .onda asset is not decoded as source");
    assert!(image
        .sources()
        .documents
        .iter()
        .all(|document| document.path != "assets/data.onda"));
    assert!(image.buffer_bindings().is_empty());
}

#[test]
fn materialized_workspaces_reject_cross_manifest_source_asset_role_conflicts() {
    let selected_manifest = ProjectManifest::empty("selected.onda")
        .to_pretty_json()
        .expect("selected manifest JSON")
        .into_bytes();
    let mut other_manifest = ProjectManifest::empty("other.onda");
    other_manifest.buffers.insert(
        "data".to_owned(),
        serde_json::from_value(json!({ "file": "selected.onda" }))
            .expect("conflicting buffer binding"),
    );
    let files = BTreeMap::from([
        ("selected.ondaproject".to_owned(), selected_manifest),
        (
            "other.ondaproject".to_owned(),
            other_manifest
                .to_pretty_json()
                .expect("other manifest JSON")
                .into_bytes(),
        ),
        (
            "selected.onda".to_owned(),
            b"outs 1\nsample:\n  out1 = 0.0\n".to_vec(),
        ),
        ("other.onda".to_owned(), b"buffers:\n  data: f32\n".to_vec()),
    ]);

    let error = ProjectImage::from_materialized_files_with_manifest(
        &files,
        "selected.ondaproject",
        ProjectLimits::default(),
    )
    .expect_err("one workspace path cannot be both source entry and buffer asset");
    assert!(error
        .to_string()
        .contains("selected project entry 'selected.onda' is also claimed as a buffer asset"));
}

#[test]
fn materialized_projects_enforce_the_total_buffer_limit_while_loading() {
    let mut manifest = ProjectManifest::empty("main.onda");
    for (name, value) in [("first", 1), ("second", 2)] {
        manifest.buffers.insert(
            name.to_owned(),
            serde_json::from_value(json!({
                "inline": {
                    "element": "i32",
                    "channels": 1,
                    "sample_rate": 1,
                    "values": [value]
                }
            }))
            .expect("inline binding"),
        );
    }
    let files = BTreeMap::from([
        (
            ONDA_PROJECT_DEFAULT_FILE_NAME.to_owned(),
            manifest
                .to_pretty_json()
                .expect("manifest JSON")
                .into_bytes(),
        ),
        (
            "main.onda".to_owned(),
            b"outs 1\nsample:\n  out1 = 0.0\n".to_vec(),
        ),
    ]);
    let limits = ProjectLimits {
        max_asset_bytes: 4,
        max_total_asset_bytes: 4,
        ..ProjectLimits::default()
    };

    ProjectImage::from_materialized_files(&files, limits)
        .expect_err("the second buffer must exceed the cumulative host limit");
}

#[test]
fn shared_assets_are_deduplicated_in_images_but_count_each_resident_binding() {
    let asset = BufferAsset::new(1, 1, 1.0, BufferSamples::I32(vec![42])).expect("valid asset");
    let mut manifest = ProjectManifest::empty("main.onda");
    for name in ["first", "second"] {
        manifest.buffers.insert(
            name.to_owned(),
            serde_json::from_value(json!({ "file": "assets/shared.ondabuffer" }))
                .expect("file binding"),
        );
    }
    let source = b"buffers:\n  first: i32\n  second: i32\n".to_vec();
    let asset_bytes = encode_ondabuffer(&asset).expect("encode shared asset");
    let limits = ProjectLimits {
        max_asset_bytes: asset.payload_bytes(),
        max_total_asset_bytes: asset.payload_bytes(),
        ..ProjectLimits::default()
    };
    let files = BTreeMap::from([
        (
            ONDA_PROJECT_DEFAULT_FILE_NAME.to_owned(),
            manifest
                .to_pretty_json()
                .expect("manifest JSON")
                .into_bytes(),
        ),
        ("main.onda".to_owned(), source.clone()),
        ("assets/shared.ondabuffer".to_owned(), asset_bytes.clone()),
    ]);

    let image = ProjectImage::from_materialized_files(&files, limits)
        .expect("shared materialized assets count once");
    assert_eq!(image.buffer_bindings().len(), 2);
    assert_eq!(image.assets().len(), 1);

    let directory = temporary_directory("shared-file-limit");
    fs::create_dir_all(directory.join("assets")).expect("create asset directory");
    fs::write(directory.join("main.onda"), source).expect("write source");
    fs::write(directory.join("assets/shared.ondabuffer"), asset_bytes).expect("write asset");
    let project_path = directory.join("shared.ondaproject");
    fs::write(
        &project_path,
        manifest.to_pretty_json().expect("manifest JSON"),
    )
    .expect("write manifest");
    let ProjectInput::Project(project) =
        resolve_project_input(&project_path, limits).expect("resolve filesystem project")
    else {
        panic!("expected project input");
    };
    project
        .load_buffer_assets(limits)
        .expect_err("each mutable filesystem binding requires resident payload storage");

    let resident_limits = ProjectLimits {
        max_total_asset_bytes: asset.payload_bytes() * 2,
        ..limits
    };
    let ProjectInput::Project(project) = resolve_project_input(&project_path, resident_limits)
        .expect("resolve filesystem project with resident capacity")
    else {
        panic!("expected project input");
    };
    let loaded = project
        .load_buffer_assets(resident_limits)
        .expect("shared filesystem bindings fit their resident allocation budget");
    assert_eq!(loaded.len(), 2);
    assert!(loaded.values().all(|(loaded, _)| loaded == &asset));
    fs::remove_dir_all(directory).expect("remove test directory");
}

#[test]
fn identical_canonical_files_are_deduplicated_before_decoding_against_the_total_budget() {
    let asset = BufferAsset::new(1, 1, 1.0, BufferSamples::I32(vec![42])).expect("valid asset");
    let encoded = encode_ondabuffer(&asset).expect("encode canonical asset");
    let mut manifest = ProjectManifest::empty("main.onda");
    for (name, file) in [
        ("first", "first.ondabuffer"),
        ("second", "second.ondabuffer"),
    ] {
        manifest.buffers.insert(
            name.to_owned(),
            serde_json::from_value(json!({ "file": file })).expect("file binding"),
        );
    }
    let files = BTreeMap::from([
        (
            ONDA_PROJECT_DEFAULT_FILE_NAME.to_owned(),
            manifest
                .to_pretty_json()
                .expect("manifest JSON")
                .into_bytes(),
        ),
        (
            "main.onda".to_owned(),
            b"buffers:\n  first: i32\n  second: i32\n".to_vec(),
        ),
        ("first.ondabuffer".to_owned(), encoded.clone()),
        ("second.ondabuffer".to_owned(), encoded),
    ]);
    let limits = ProjectLimits {
        max_asset_bytes: asset.payload_bytes(),
        max_total_asset_bytes: asset.payload_bytes(),
        ..ProjectLimits::default()
    };

    let image = ProjectImage::from_materialized_files(&files, limits)
        .expect("duplicate canonical content should not consume a second decode budget");
    assert_eq!(image.buffer_bindings().len(), 2);
    assert_eq!(image.assets().len(), 1);
}

#[test]
fn materialized_projects_enforce_transport_byte_limits_in_the_shared_loader() {
    let manifest = ProjectManifest::empty("main.onda")
        .to_pretty_json()
        .expect("manifest JSON")
        .into_bytes();
    let source = b"outs 1\nsample:\n  out1 = 0.0\n".to_vec();
    let limits = ProjectLimits {
        max_documents: 3,
        max_assets: 0,
        max_manifest_bytes: 128,
        max_source_bytes: 128,
        max_asset_bytes: 1,
        max_total_asset_bytes: 0,
        ..ProjectLimits::default()
    };

    let mut oversized_file = BTreeMap::from([
        (ONDA_PROJECT_DEFAULT_FILE_NAME.to_owned(), manifest.clone()),
        ("main.onda".to_owned(), source.clone()),
    ]);
    oversized_file.insert("ignored.bin".to_owned(), vec![0; 129]);
    let error = ProjectImage::from_materialized_files(&oversized_file, limits)
        .expect_err("unclaimed files must still obey the per-file transport limit");
    assert!(error.to_string().contains("byte file limit"));

    let mut oversized_total = BTreeMap::from([
        (ONDA_PROJECT_DEFAULT_FILE_NAME.to_owned(), manifest),
        ("main.onda".to_owned(), source),
    ]);
    oversized_total.insert("first.bin".to_owned(), vec![0; 110]);
    oversized_total.insert("second.bin".to_owned(), vec![0; 110]);
    let error = ProjectImage::from_materialized_files(&oversized_total, limits)
        .expect_err("unclaimed files must still obey the aggregate transport limit");
    assert!(error.to_string().contains("byte aggregate limit"));
}

#[test]
fn ondaproject_file_resolves_entry_and_typed_inline_buffers() {
    let directory = temporary_directory("manifest");
    fs::create_dir_all(directory.join("src")).expect("create source directory");
    fs::write(directory.join("src/main.onda"), "buffers:\n  values: i32\n").expect("write source");
    let mut manifest = ProjectManifest::empty("src/main.onda");
    manifest.buffers.insert(
        "values".to_owned(),
        serde_json::from_value(json!({
            "inline": {
                "element": "i32",
                "channels": 1,
                "sample_rate": 1,
                "values": [1, 2, 3]
            }
        }))
        .expect("inline binding"),
    );
    let project_path = directory.join("synth.ondaproject");
    fs::write(
        &project_path,
        manifest.to_pretty_json().expect("manifest JSON"),
    )
    .expect("write project file");

    let ProjectInput::Project(project) =
        resolve_project_input(&project_path, ProjectLimits::default()).expect("resolve project")
    else {
        panic!("expected .ondaproject input");
    };
    let buffers = project
        .load_buffer_assets(ProjectLimits::default())
        .expect("load inline buffer");
    assert_eq!(
        buffers["values"].0.samples,
        BufferSamples::I32(vec![1, 2, 3])
    );
    resolve_project_input(&directory, ProjectLimits::default())
        .expect_err("directories are not project entry points");
    fs::remove_dir_all(directory).expect("remove test directory");
}

#[test]
fn project_constants_round_trip_through_images_and_materialization() {
    let manifest: ProjectManifest = serde_json::from_value(json!({
        "entry": "main.onda",
        "constants": {
            "Enabled": true,
            "Count": 8,
            "Wide": "9007199254740993",
            "Window": [0.0, 0.5, 1.0]
        }
    }))
    .expect("parse project constants");
    manifest
        .validate(&ProjectLimits::default())
        .expect("validate project constants");
    assert_eq!(manifest.constants["Enabled"], ProjectConstValue::Bool(true));
    assert_eq!(
        manifest.constants["Wide"].onda_literal(),
        "i64(9007199254740993)"
    );

    let files = BTreeMap::from([
        (
            ONDA_PROJECT_DEFAULT_FILE_NAME.to_owned(),
            manifest
                .to_pretty_json()
                .expect("manifest JSON")
                .into_bytes(),
        ),
        (
            "main.onda".to_owned(),
            b"config const Enabled: bool = false\nconfig const Count: i32 = 0\nconfig const Wide: i64 = i64(0)\nconfig const Window: f64[] = []\nsample:\n  out1 = 0.0\n"
                .to_vec(),
        ),
    ]);
    let image = ProjectImage::from_materialized_files(&files, ProjectLimits::default())
        .expect("load project constants into image");
    assert_eq!(image.constants(), &manifest.constants);

    let serialized = image.serialize().expect("serialize project image");
    let restored = ProjectImage::deserialize(&serialized, ProjectLimits::default())
        .expect("deserialize project image");
    assert_eq!(restored.constants(), &manifest.constants);

    let plan = restored.materialization_plan().expect("materialize image");
    let manifest_file = plan
        .files
        .iter()
        .find(|file| file.relative_path == ONDA_PROJECT_DEFAULT_FILE_NAME)
        .expect("materialized manifest");
    let materialized: ProjectManifest =
        serde_json::from_slice(&manifest_file.bytes).expect("parse materialized manifest");
    assert_eq!(materialized.constants, manifest.constants);
}

#[test]
fn project_watch_paths_preserve_missing_assets() {
    let directory = temporary_directory("missing-watch-asset");
    fs::create_dir_all(directory.join("assets")).expect("create asset directory");
    fs::write(
        directory.join("main.onda"),
        "outs 1\nsample:\n  out1 = 0.0\n",
    )
    .expect("write source");
    let mut manifest = ProjectManifest::empty("main.onda");
    manifest.buffers.insert(
        "sample".to_owned(),
        serde_json::from_value(json!({ "file": "assets/missing.ondabuffer" }))
            .expect("file binding"),
    );
    let project_path = directory.join("project.ondaproject");
    fs::write(
        &project_path,
        manifest.to_pretty_json().expect("manifest JSON"),
    )
    .expect("write manifest");

    let ProjectInput::Project(project) =
        resolve_project_input(&project_path, ProjectLimits::default()).expect("resolve project")
    else {
        panic!("expected project input");
    };
    let watch_paths = project
        .watch_paths()
        .expect("missing assets must remain watchable");
    assert_eq!(
        watch_paths.assets,
        vec![project.root.join("assets/missing.ondabuffer")]
    );

    fs::remove_dir_all(directory).expect("remove test directory");
}

#[test]
fn project_watch_paths_preserve_a_missing_entry() {
    let directory = temporary_directory("missing-watch-entry");
    fs::create_dir_all(directory.join("code")).expect("create source directory");
    let project_path = directory.join("project.ondaproject");
    fs::write(
        &project_path,
        ProjectManifest::empty("code/new.onda")
            .to_pretty_json()
            .expect("manifest JSON"),
    )
    .expect("write manifest");

    resolve_project_input(&project_path, ProjectLimits::default())
        .expect_err("ordinary project resolution must still require an entry file");
    let watch_paths = resolve_project_watch_paths(&project_path, ProjectLimits::default())
        .expect("the missing entry must remain watchable");
    assert_eq!(watch_paths.entry, directory.join("code/new.onda"));

    fs::remove_dir_all(directory).expect("remove test directory");
}

#[cfg(unix)]
#[test]
fn project_paths_reject_symlinks() {
    use std::os::unix::fs::symlink;

    let directory = temporary_directory("symlink-watch-asset");
    fs::create_dir_all(directory.join("assets")).expect("create asset directory");
    fs::create_dir_all(directory.join("media")).expect("create media directory");
    fs::write(
        directory.join("media/main.onda"),
        "outs 1\nsample:\n  out1 = 0.0\n",
    )
    .expect("write source");
    symlink("media/main.onda", directory.join("main.onda")).expect("create entry symlink");
    let target = directory.join("media/sample.ondabuffer");
    fs::write(&target, [1_u8, 2, 3]).expect("write asset");
    symlink(
        "../media/sample.ondabuffer",
        directory.join("assets/sample.ondabuffer"),
    )
    .expect("create asset symlink");
    let mut manifest = ProjectManifest::empty("main.onda");
    manifest.buffers.insert(
        "sample".to_owned(),
        serde_json::from_value(json!({ "file": "assets/sample.ondabuffer" }))
            .expect("file binding"),
    );
    let project_path = directory.join("project.ondaproject");
    fs::write(
        &project_path,
        manifest.to_pretty_json().expect("manifest JSON"),
    )
    .expect("write manifest");

    let error = resolve_project_input(&project_path, ProjectLimits::default())
        .expect_err("project entry symlinks must be rejected");
    assert!(error.to_string().contains("symlink component"));
    let error = resolve_project_input(directory.join("main.onda"), ProjectLimits::default())
        .expect_err("standalone source inputs must reject symlinks too");
    assert!(error.to_string().contains("symlink component"));

    fs::remove_file(directory.join("main.onda")).expect("remove entry symlink");
    fs::write(
        directory.join("main.onda"),
        "outs 1\nsample:\n  out1 = 0.0\n",
    )
    .expect("write regular entry");
    let ProjectInput::Project(project) =
        resolve_project_input(&project_path, ProjectLimits::default()).expect("resolve project")
    else {
        panic!("expected project input");
    };
    let error = project
        .watch_paths()
        .expect_err("project asset symlinks must be rejected");
    assert!(error.to_string().contains("symlink component"));
    let recovery_paths = resolve_project_watch_paths(&project_path, ProjectLimits::default())
        .expect("recovery watches must retain the unsupported asset alias");
    assert_eq!(
        recovery_paths.assets,
        vec![directory.join("assets/sample.ondabuffer")]
    );

    let manifest_alias = directory.join("linked.ondaproject");
    symlink(&project_path, &manifest_alias).expect("create manifest symlink");
    let error = resolve_project_input(&manifest_alias, ProjectLimits::default())
        .expect_err("project manifest symlinks must be rejected");
    assert!(error.to_string().contains("symlink component"));

    fs::remove_dir_all(directory).expect("remove test directory");
}

#[cfg(unix)]
#[test]
fn project_watch_paths_reject_missing_assets_below_external_symlinks() {
    use std::os::unix::fs::symlink;

    let directory = temporary_directory("external-symlink-watch-asset");
    let project_directory = directory.join("project");
    let external_directory = directory.join("external");
    fs::create_dir_all(&project_directory).expect("create project directory");
    fs::create_dir_all(&external_directory).expect("create external directory");
    fs::write(
        project_directory.join("main.onda"),
        "outs 1\nsample:\n  out1 = 0.0\n",
    )
    .expect("write source");
    symlink(&external_directory, project_directory.join("assets"))
        .expect("create external asset symlink");
    let mut manifest = ProjectManifest::empty("main.onda");
    manifest.buffers.insert(
        "sample".to_owned(),
        serde_json::from_value(json!({ "file": "assets/missing.ondabuffer" }))
            .expect("file binding"),
    );
    let project_path = project_directory.join("project.ondaproject");
    fs::write(
        &project_path,
        manifest.to_pretty_json().expect("manifest JSON"),
    )
    .expect("write manifest");

    let ProjectInput::Project(project) =
        resolve_project_input(&project_path, ProjectLimits::default()).expect("resolve project")
    else {
        panic!("expected project input");
    };
    let error = project
        .watch_paths()
        .expect_err("external symlinks must remain confined");
    assert!(error.to_string().contains("symlink component"));

    fs::remove_dir_all(directory).expect("remove test directory");
}

#[cfg(target_os = "linux")]
#[test]
fn filesystem_projects_reject_manifest_name_portability_collisions() {
    let directory = temporary_directory("manifest-name-collision");
    fs::create_dir_all(&directory).expect("create project directory");
    fs::write(
        directory.join("SESSION.ONDAPROJECT"),
        "outs 1\nsample:\n  out1 = 0.0\n",
    )
    .expect("write case-distinct entry");
    let project_path = directory.join("session.ondaproject");
    fs::write(
        &project_path,
        ProjectManifest::empty("SESSION.ONDAPROJECT")
            .to_pretty_json()
            .expect("manifest JSON"),
    )
    .expect("write project manifest");

    let error = resolve_project_input(&project_path, ProjectLimits::default())
        .expect_err("manifest and entry must not collide on case-insensitive hosts");
    assert!(error
        .to_string()
        .contains("cannot also be a referenced source or buffer asset"));
    fs::remove_dir_all(directory).expect("remove test directory");
}

#[test]
fn ondaproject_buffer_arrays_preserve_nullable_slots() {
    let directory = temporary_directory("manifest-buffer-array");
    fs::create_dir_all(&directory).expect("create project directory");
    fs::write(directory.join("main.onda"), "buffers:\n  bank: f32 {3}\n").expect("write source");
    let manifest: ProjectManifest = serde_json::from_value(json!({
        "entry": "main.onda",
        "buffers": {
            "bank": [
                { "inline": { "element": "f32", "channels": 1, "sample_rate": 48000, "values": [0.25] } },
                null,
                { "inline": { "element": "f32", "channels": 1, "sample_rate": 48000, "values": [0.75] } }
            ]
        }
    }))
    .expect("array manifest");
    let project_path = directory.join("bank.ondaproject");
    fs::write(
        &project_path,
        manifest.to_pretty_json().expect("manifest JSON"),
    )
    .expect("write manifest");

    let ProjectInput::Project(project) =
        resolve_project_input(&project_path, ProjectLimits::default()).expect("resolve project")
    else {
        panic!("expected project input");
    };
    let buffers = project
        .load_buffer_assets(ProjectLimits::default())
        .expect("load buffer array");
    assert_eq!(buffers.len(), 2);
    assert_eq!(buffers["bank[0]"].0.samples, BufferSamples::F32(vec![0.25]));
    assert_eq!(buffers["bank[2]"].0.samples, BufferSamples::F32(vec![0.75]));
    validate_buffer_assets(
        buffers
            .iter()
            .map(|(name, (asset, _))| (name.as_str(), asset)),
        &[ProjectBufferDeclaration {
            name: "bank".to_owned(),
            element: BufferElement::F32,
            channels: ProjectBufferChannels::Mono,
            array_len: 3,
            is_array: true,
        }],
    )
    .expect("array slots match declaration");
    fs::remove_dir_all(directory).expect("remove test directory");
}

#[test]
fn project_buffer_overrides_exclude_individual_array_slots_before_loading() {
    let directory = temporary_directory("manifest-buffer-array-override");
    fs::create_dir_all(&directory).expect("create project directory");
    fs::write(directory.join("main.onda"), "buffers:\n  bank: f32 {2}\n").expect("write source");
    let manifest: ProjectManifest = serde_json::from_value(json!({
        "entry": "main.onda",
        "buffers": {
            "bank": [
                { "file": "missing.ondabuffer" },
                { "inline": { "element": "f32", "channels": 1, "sample_rate": 48000, "values": [0.75] } }
            ]
        }
    }))
    .expect("array manifest");
    let project_path = directory.join("bank.ondaproject");
    fs::write(
        &project_path,
        manifest.to_pretty_json().expect("manifest JSON"),
    )
    .expect("write manifest");

    let ProjectInput::Project(project) =
        resolve_project_input(&project_path, ProjectLimits::default()).expect("resolve project")
    else {
        panic!("expected project input");
    };
    let buffers = project
        .load_buffer_assets_excluding(ProjectLimits::default(), &BTreeSet::from(["bank[0]"]))
        .expect("the overridden missing slot must not be loaded");
    assert_eq!(buffers.len(), 1);
    assert_eq!(buffers["bank[1]"].0.samples, BufferSamples::F32(vec![0.75]));
    fs::remove_dir_all(directory).expect("remove test directory");
}

#[test]
fn ondaproject_buffer_arrays_reject_flat_slot_keys() {
    let manifest: ProjectManifest = serde_json::from_value(json!({
        "entry": "main.onda",
        "buffers": {
            "bank[0]": {
                "inline": {
                    "element": "f32",
                    "channels": 1,
                    "sample_rate": 48000,
                    "values": [0.25]
                }
            }
        }
    }))
    .expect("structurally valid manifest JSON");

    let error = manifest
        .validate(&ProjectLimits::default())
        .expect_err("array slots must use the canonical manifest array form");
    assert!(error.to_string().contains("use an array binding"));
}

#[test]
fn project_manifests_reject_nonportable_referenced_file_collisions() {
    let file_binding =
        |path: &str| serde_json::from_value(json!({ "file": path })).expect("file binding");

    let mut manifest = ProjectManifest::empty("src/main.onda");
    manifest
        .buffers
        .insert("first".to_owned(), file_binding("Assets/sample.ondabuffer"));
    manifest.buffers.insert(
        "second".to_owned(),
        file_binding("assets/SAMPLE.ondabuffer"),
    );
    manifest
        .validate(&ProjectLimits::default())
        .expect_err("case-insensitive asset collisions must fail");

    let mut manifest = ProjectManifest::empty("src/Main.onda");
    manifest
        .buffers
        .insert("sample".to_owned(), file_binding("src/main.onda"));
    manifest
        .validate(&ProjectLimits::default())
        .expect_err("entry and asset collisions must fail");

    let mut manifest = ProjectManifest::empty("src/main.onda");
    manifest
        .buffers
        .insert("first".to_owned(), file_binding("assets/shared.ondabuffer"));
    manifest.buffers.insert(
        "second".to_owned(),
        file_binding("assets/shared.ondabuffer"),
    );
    manifest
        .validate(&ProjectLimits::default())
        .expect("multiple buffers may share the same exact asset file");
}

#[test]
fn generated_ondaproject_files_contain_only_project_data() {
    let manifest = ProjectManifest::empty("src/main.onda");
    assert_eq!(
        manifest.to_pretty_json().expect("project JSON"),
        "{\n  \"entry\": \"src/main.onda\"\n}\n"
    );
}

#[cfg(unix)]
#[test]
fn source_capture_input_rejects_symlink_paths() {
    use std::os::unix::fs::symlink;

    let root = temporary_directory("symlink-capture");
    let real_root = root.join("real-project");
    let linked_root = root.join("linked-project");
    fs::create_dir_all(&real_root).expect("create real project directory");
    fs::write(
        real_root.join("main.onda"),
        "include \"voice.onda\"\nouts 1\nsample:\n  out1 = LEVEL\n",
    )
    .expect("write entry");
    fs::write(real_root.join("voice.onda"), "const LEVEL = 0.25\n").expect("write dependency");
    symlink(&real_root, &linked_root).expect("create project symlink");

    let linked_entry = linked_root.join("main.onda");
    let error = onda_frontend::load_program_file(&linked_entry)
        .expect_err("filesystem source capture must reject symlink paths");
    assert!(error.diagnostics[0].message.contains("symlink component"));

    fs::remove_dir_all(root).expect("remove symlink capture directory");
}

#[test]
fn source_capture_relocates_and_rewrites_the_exact_graph() {
    use onda_frontend::{
        SourceDocument as FrontendDocument, SourceManifest,
        SourceReferenceKind as FrontendReferenceKind, SourceResolution as FrontendResolution,
    };

    let root = PathBuf::from("/workspace/project");
    let entry = root.join("src/main.onda");
    let local = root.join("shared/common.onda");
    let external = PathBuf::from("/vendor/math.onda");
    let manifest = SourceManifest {
        files: vec![entry.clone(), local.clone(), external.clone()],
        documents: vec![
            FrontendDocument {
                path: entry.clone(),
                contents: "include \"../shared/common.onda\"\nimport vendor\n".to_owned(),
            },
            FrontendDocument {
                path: local.clone(),
                contents: "const common = 1\n".to_owned(),
            },
            FrontendDocument {
                path: external.clone(),
                contents: "const external = 2\n".to_owned(),
            },
        ]
        .into_boxed_slice(),
        resolutions: vec![
            FrontendResolution {
                source: entry.clone(),
                kind: FrontendReferenceKind::Include,
                specifier: "../shared/common.onda".to_owned(),
                target: local,
            },
            FrontendResolution {
                source: entry.clone(),
                kind: FrontendReferenceKind::Import,
                specifier: "vendor".to_owned(),
                target: external,
            },
        ]
        .into_boxed_slice(),
        ..SourceManifest::default()
    };

    let captured = SourceImage::capture(&entry, &root, &manifest, ProjectLimits::default())
        .expect("capture source graph");
    assert_eq!(captured.entry, "src/main.onda");
    let entry_source = captured
        .documents
        .iter()
        .find(|document| document.path == captured.entry)
        .expect("captured entry");
    assert_eq!(
        entry_source.contents,
        "include \"../shared/common.onda\"\nimport ../external/math\n"
    );
    assert!(captured
        .documents
        .iter()
        .any(|document| document.path == "external/math.onda"));
    assert_eq!(captured.resolutions.len(), 2);
}

#[test]
fn source_capture_normalizes_portable_output_paths_to_nfc() {
    use onda_frontend::{SourceDocument as FrontendDocument, SourceManifest};

    let root = PathBuf::from("/workspace/project");
    let entry = root.join("e\u{301}.onda");
    let manifest = SourceManifest {
        files: vec![entry.clone()],
        documents: vec![FrontendDocument {
            path: entry.clone(),
            contents: "outs 1\nsample:\n  out1 = 0.0\n".to_owned(),
        }]
        .into_boxed_slice(),
        ..SourceManifest::default()
    };

    let captured = SourceImage::capture(&entry, &root, &manifest, ProjectLimits::default())
        .expect("capture source with a decomposed filesystem name");
    assert_eq!(captured.entry, "\u{e9}.onda");
    assert_eq!(captured.documents[0].path, "\u{e9}.onda");
}

#[test]
fn source_capture_resolves_file_and_descendant_collisions_after_sanitizing() {
    use onda_frontend::{
        SourceDocument as FrontendDocument, SourceManifest,
        SourceReferenceKind as FrontendReferenceKind, SourceResolution as FrontendResolution,
    };

    let root = PathBuf::from("/workspace/project");
    let entry = root.join("main.onda");
    let sanitized_file = root.join("foo?.onda");
    let descendant = root.join("foo_.onda/bar.onda");
    let manifest = SourceManifest {
        files: vec![entry.clone(), sanitized_file.clone(), descendant.clone()],
        documents: vec![
            FrontendDocument {
                path: entry.clone(),
                contents: concat!(
                    "include \"foo?.onda\"\n",
                    "include \"foo_.onda/bar.onda\"\n",
                    "outs 1\n",
                    "sample:\n",
                    "  out1 = FIRST + SECOND\n",
                )
                .to_owned(),
            },
            FrontendDocument {
                path: sanitized_file.clone(),
                contents: "const FIRST = 1.0\n".to_owned(),
            },
            FrontendDocument {
                path: descendant.clone(),
                contents: "const SECOND = 2.0\n".to_owned(),
            },
        ]
        .into_boxed_slice(),
        resolutions: vec![
            FrontendResolution {
                source: entry.clone(),
                kind: FrontendReferenceKind::Include,
                specifier: "foo?.onda".to_owned(),
                target: sanitized_file,
            },
            FrontendResolution {
                source: entry.clone(),
                kind: FrontendReferenceKind::Include,
                specifier: "foo_.onda/bar.onda".to_owned(),
                target: descendant,
            },
        ]
        .into_boxed_slice(),
        ..SourceManifest::default()
    };

    let captured = SourceImage::capture(&entry, &root, &manifest, ProjectLimits::default())
        .expect("sanitized file-tree collisions should be relocated");
    let paths = captured
        .documents
        .iter()
        .map(|document| document.path.as_str())
        .collect::<BTreeSet<_>>();
    assert!(paths.contains("foo_.onda"));
    assert!(paths.contains("foo_-2.onda/bar.onda"));
    captured
        .replay(ProjectLimits::default())
        .expect("relocated source graph should replay exactly");
}

#[test]
fn source_capture_sanitizes_unicode_windows_device_names() {
    use onda_frontend::{SourceDocument as FrontendDocument, SourceManifest};

    let root = PathBuf::from("/workspace/project");
    let entry = root.join("COM¹.onda");
    let manifest = SourceManifest {
        files: vec![entry.clone()],
        documents: vec![FrontendDocument {
            path: entry.clone(),
            contents: "outs 1\nsample:\n  out1 = 0.0\n".to_owned(),
        }]
        .into_boxed_slice(),
        ..SourceManifest::default()
    };

    let captured = SourceImage::capture(&entry, &root, &manifest, ProjectLimits::default())
        .expect("Windows device source name should be made portable");
    assert_eq!(captured.entry, "_COM¹.onda");
}

#[cfg(unix)]
#[test]
fn source_capture_sanitizes_backslashes_in_filesystem_components() {
    use onda_frontend::{SourceDocument as FrontendDocument, SourceManifest};

    let root = PathBuf::from("/workspace/project");
    let entry = root.join(r"voices\lead.onda");
    let manifest = SourceManifest {
        files: vec![entry.clone()],
        documents: vec![FrontendDocument {
            path: entry.clone(),
            contents: "outs 1\nsample:\n  out1 = 0.0\n".to_owned(),
        }]
        .into_boxed_slice(),
        ..SourceManifest::default()
    };

    let captured = SourceImage::capture(&entry, &root, &manifest, ProjectLimits::default())
        .expect("capture source whose Unix filename contains a backslash");
    assert_eq!(captured.entry, "voices_lead.onda");
    assert_eq!(captured.documents[0].path, "voices_lead.onda");
}

#[test]
fn source_capture_canonicalizes_module_extensions_for_case_sensitive_hosts() {
    use onda_frontend::{
        SourceDocument as FrontendDocument, SourceManifest,
        SourceReferenceKind as FrontendReferenceKind, SourceResolution as FrontendResolution,
    };

    let root = PathBuf::from("/workspace/project");
    let entry = root.join("main.onda");
    let module = root.join("module.ONDA");
    let manifest = SourceManifest {
        files: vec![entry.clone(), module.clone()],
        documents: vec![
            FrontendDocument {
                path: entry.clone(),
                contents: "import module\n".to_owned(),
            },
            FrontendDocument {
                path: module.clone(),
                contents: "const imported = 1.0\n".to_owned(),
            },
        ]
        .into_boxed_slice(),
        resolutions: vec![FrontendResolution {
            source: entry.clone(),
            kind: FrontendReferenceKind::Import,
            specifier: "module".to_owned(),
            target: module,
        }]
        .into_boxed_slice(),
        ..SourceManifest::default()
    };

    let captured = SourceImage::capture(&entry, &root, &manifest, ProjectLimits::default())
        .expect("capture source graph from a case-insensitive filesystem");
    assert!(captured
        .documents
        .iter()
        .any(|document| document.path == "module.onda"));
    assert_eq!(captured.resolutions[0].target, "module.onda");
    assert_eq!(captured.resolutions[0].specifier, "module");

    let image = ProjectImage::from_buffer_assets(captured, BTreeMap::new())
        .expect("captured graph should form a project image");
    let files = image
        .materialization_plan()
        .expect("materialize captured graph")
        .files
        .into_iter()
        .map(|file| (file.relative_path, file.bytes))
        .collect::<BTreeMap<_, _>>();
    ProjectImage::from_materialized_files(&files, ProjectLimits::default())
        .expect("materialized graph should load on a case-sensitive host");
}

fn temporary_directory(label: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "onda-project-{label}-{}-{stamp}",
        std::process::id()
    ))
}

fn pcm16_wav(samples: &[i16]) -> Vec<u8> {
    let data_bytes = u32::try_from(samples.len() * 2).expect("test WAV size fits u32");
    let mut bytes = Vec::with_capacity(44 + data_bytes as usize);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data_bytes).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&48_000u32.to_le_bytes());
    bytes.extend_from_slice(&96_000u32.to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_bytes.to_le_bytes());
    for sample in samples {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    bytes
}
