use std::alloc::{alloc, dealloc, Layout};
use std::ffi::{c_char, c_void, CStr, CString};
use std::fs;
use std::ops::{Deref, DerefMut};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread::ThreadId;
use std::time::{SystemTime, UNIX_EPOCH};

use onda::*;

fn diag_message(diag: &onda_diag_t) -> String {
    if diag.message.is_null() {
        return "<null>".to_owned();
    }
    unsafe { CStr::from_ptr(diag.message).to_string_lossy().into_owned() }
}

struct DiagnosticHandle(onda_diag_t);

impl DiagnosticHandle {
    fn clear(&mut self) {
        unsafe {
            onda_diag_dispose(&mut self.0);
        }
    }
}

impl Deref for DiagnosticHandle {
    type Target = onda_diag_t;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for DiagnosticHandle {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl Drop for DiagnosticHandle {
    fn drop(&mut self) {
        self.clear();
    }
}

fn empty_diag() -> DiagnosticHandle {
    DiagnosticHandle(onda_diag_t {
        code: 0,
        line: 0,
        column: 0,
        end_line: 0,
        end_column: 0,
        message: std::ptr::null(),
        file: std::ptr::null(),
        trace: std::ptr::null(),
    })
}

struct ProgramHandle(*mut onda_program);

impl Drop for ProgramHandle {
    fn drop(&mut self) {
        unsafe {
            onda_program_destroy(self.0);
        }
    }
}

struct SourceManifestHandle(*mut onda_source_manifest);

impl Drop for SourceManifestHandle {
    fn drop(&mut self) {
        unsafe {
            onda_source_manifest_destroy(self.0);
        }
    }
}

struct ProjectImageHandle(*mut onda_project_image);

impl Drop for ProjectImageHandle {
    fn drop(&mut self) {
        unsafe {
            onda_project_image_destroy(self.0);
        }
    }
}

struct ProjectPlanHandle(*mut onda_project_materialization_plan);

impl Drop for ProjectPlanHandle {
    fn drop(&mut self) {
        unsafe {
            onda_project_materialization_destroy(self.0);
        }
    }
}

struct InstanceHandle(*mut onda_instance);

impl Drop for InstanceHandle {
    fn drop(&mut self) {
        unsafe {
            onda_instance_destroy(self.0);
        }
    }
}

#[derive(Default)]
struct AllocStats {
    allocs: usize,
    frees: usize,
    live: usize,
}

unsafe extern "C" fn test_alloc(context: *mut c_void, size: usize, align: usize) -> *mut c_void {
    let stats = &mut *(context.cast::<AllocStats>());
    let Ok(layout) = Layout::from_size_align(size, align) else {
        return std::ptr::null_mut();
    };
    let ptr = alloc(layout).cast::<c_void>();
    if !ptr.is_null() {
        stats.allocs += 1;
        stats.live += 1;
    }
    ptr
}

unsafe extern "C" fn test_free(context: *mut c_void, ptr: *mut c_void, size: usize, align: usize) {
    let stats = &mut *(context.cast::<AllocStats>());
    let layout = Layout::from_size_align(size, align).expect("valid free layout");
    dealloc(ptr.cast::<u8>(), layout);
    stats.frees += 1;
    stats.live -= 1;
}

struct ThreadBoundAllocStats {
    owner: ThreadId,
    allocs: AtomicUsize,
    frees: AtomicUsize,
    rejected_foreign_allocs: AtomicUsize,
    foreign_frees: AtomicUsize,
}

impl ThreadBoundAllocStats {
    fn new() -> Self {
        Self {
            owner: std::thread::current().id(),
            allocs: AtomicUsize::new(0),
            frees: AtomicUsize::new(0),
            rejected_foreign_allocs: AtomicUsize::new(0),
            foreign_frees: AtomicUsize::new(0),
        }
    }
}

unsafe extern "C" fn thread_bound_alloc(
    context: *mut c_void,
    size: usize,
    align: usize,
) -> *mut c_void {
    let stats = &*(context.cast::<ThreadBoundAllocStats>());
    if std::thread::current().id() != stats.owner {
        stats
            .rejected_foreign_allocs
            .fetch_add(1, Ordering::Relaxed);
        return std::ptr::null_mut();
    }
    let Ok(layout) = Layout::from_size_align(size, align) else {
        return std::ptr::null_mut();
    };
    let ptr = alloc(layout).cast::<c_void>();
    if !ptr.is_null() {
        stats.allocs.fetch_add(1, Ordering::Relaxed);
    }
    ptr
}

unsafe extern "C" fn thread_bound_free(
    context: *mut c_void,
    ptr: *mut c_void,
    size: usize,
    align: usize,
) {
    let stats = &*(context.cast::<ThreadBoundAllocStats>());
    if std::thread::current().id() != stats.owner {
        stats.foreign_frees.fetch_add(1, Ordering::Relaxed);
    }
    let layout = Layout::from_size_align(size, align).expect("valid free layout");
    dealloc(ptr.cast::<u8>(), layout);
    stats.frees.fetch_add(1, Ordering::Relaxed);
}

struct TransferredInstance(*mut onda_instance);

// SAFETY: the test gives the handle one exclusive owner and its allocator
// explicitly supports destruction from the receiving thread.
unsafe impl Send for TransferredInstance {}

impl TransferredInstance {
    fn into_raw(self) -> *mut onda_instance {
        self.0
    }
}

unsafe fn compile_program(src: &str) -> ProgramHandle {
    let src_c = CString::new(src).expect("source contains no NUL bytes");
    let options = onda_compile_options_t {
        fast_math: 0,
        sample_rate: 48_000.0,
        block_size: 512,
    };
    let mut diag = empty_diag();
    let program = onda_compile(src_c.as_ptr(), &options, &mut *diag);
    assert!(
        !program.is_null(),
        "compile failed: {}",
        diag_message(&diag)
    );
    ProgramHandle(program)
}

#[test]
fn diagnostic_strings_have_an_explicit_reusable_lifecycle() {
    unsafe {
        let invalid = CString::new("this is not valid Onda").unwrap();
        let valid = CString::new("outs { out1 }\nsample { out1 = 0.0 }\n").unwrap();
        let options = onda_compile_options_t {
            fast_math: 0,
            sample_rate: 48_000.0,
            block_size: 64,
        };
        let mut diag = empty_diag();

        for _ in 0..32 {
            let program = onda_compile(invalid.as_ptr(), &options, &mut *diag);
            assert!(program.is_null());
            assert!(!diag.message.is_null());
            assert_ne!(diag.code, 0);
            diag.clear();
            assert!(diag.message.is_null());
            assert!(diag.file.is_null());
            assert!(diag.trace.is_null());
            assert_eq!(diag.code, 0);
        }

        let program = onda_compile(valid.as_ptr(), &options, &mut *diag);
        assert!(
            !program.is_null(),
            "compile failed: {}",
            diag_message(&diag)
        );
        let _program = ProgramHandle(program);
        diag.clear();
        diag.clear();
        onda_diag_dispose(std::ptr::null_mut());
    }
}

#[test]
fn null_diagnostic_output_releases_generated_strings() {
    unsafe {
        let invalid = CString::new("this is not valid Onda").unwrap();
        let options = onda_compile_options_t {
            fast_math: 0,
            sample_rate: 48_000.0,
            block_size: 64,
        };
        for _ in 0..32 {
            assert!(onda_compile(invalid.as_ptr(), &options, std::ptr::null_mut()).is_null());
        }
    }
}

fn temp_source_dir(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("onda_api_{prefix}_{nanos}"));
    fs::create_dir_all(&path).expect("create temp source directory");
    path
}

unsafe fn manifest_paths(manifest: *const onda_source_manifest) -> Vec<PathBuf> {
    (0..onda_source_manifest_count(manifest))
        .map(|index| {
            PathBuf::from(
                CStr::from_ptr(onda_source_manifest_path(manifest, index))
                    .to_str()
                    .expect("source path should be UTF-8"),
            )
        })
        .collect()
}

unsafe fn manifest_unresolved_paths(manifest: *const onda_source_manifest) -> Vec<PathBuf> {
    (0..onda_source_manifest_unresolved_count(manifest))
        .map(|index| {
            PathBuf::from(
                CStr::from_ptr(onda_source_manifest_unresolved_path(manifest, index))
                    .to_str()
                    .expect("unresolved source path should be UTF-8"),
            )
        })
        .collect()
}

unsafe fn manifest_watch_paths(manifest: *const onda_source_manifest) -> Vec<PathBuf> {
    (0..onda_source_manifest_watch_count(manifest))
        .map(|index| {
            PathBuf::from(
                CStr::from_ptr(onda_source_manifest_watch_path(manifest, index))
                    .to_str()
                    .expect("watch path should be UTF-8"),
            )
        })
        .collect()
}

#[test]
fn c_file_compile_returns_source_manifest_on_success_and_failure() {
    unsafe {
        let dir = temp_source_dir("source_manifest");
        let main = dir.join("main.onda");
        let shared = dir.join("shared.onda");
        let nested = dir.join("nested.onda");
        let dependency = dir.join("dependency.onda");
        fs::write(
            &main,
            "include \"shared.onda\"\nimport dependency\nouts 1\nsample:\n  out1 = dependency_value()\n",
        )
        .expect("write entry");
        fs::write(&shared, "import nested\nconst SHARED = 1.0\n").expect("write include");
        fs::write(&nested, "const NESTED = 2.0\n").expect("write nested import");
        fs::write(
            &dependency,
            "def dependency_value() -> f32:\n  return 0.5\n",
        )
        .expect("write dependency");

        let path = CString::new(main.to_str().expect("UTF-8 entry path")).expect("C entry path");
        let options = onda_compile_options_t {
            fast_math: 0,
            sample_rate: 48_000.0,
            block_size: 64,
        };
        let mut diag = empty_diag();
        let mut manifest = std::ptr::null_mut();
        let program = onda_compile_file(path.as_ptr(), &options, &mut manifest, &mut *diag);
        assert!(
            !program.is_null(),
            "compile failed: {}",
            diag_message(&diag)
        );
        let _program = ProgramHandle(program);
        let manifest = SourceManifestHandle(manifest);
        assert_eq!(
            manifest_paths(manifest.0),
            vec![
                fs::canonicalize(&main).expect("canonical entry"),
                fs::canonicalize(&shared).expect("canonical include"),
                fs::canonicalize(&nested).expect("canonical nested import"),
                fs::canonicalize(&dependency).expect("canonical dependency"),
            ]
        );
        assert_eq!(manifest_watch_paths(manifest.0), manifest_paths(manifest.0));
        assert!(manifest_unresolved_paths(manifest.0).is_empty());

        let replay_sources = (0..onda_source_manifest_document_count(manifest.0))
            .map(|index| {
                let mut source_bytes = 0;
                onda_source_graph_document_t {
                    path_utf8: onda_source_manifest_document_path(manifest.0, index),
                    source_utf8: onda_source_manifest_document_contents(
                        manifest.0,
                        index,
                        &mut source_bytes,
                    ),
                    source_bytes,
                }
            })
            .collect::<Vec<_>>();
        let replay_resolutions = (0..onda_source_manifest_resolution_count(manifest.0))
            .map(|index| onda_source_graph_resolution_t {
                source_path_utf8: onda_source_manifest_resolution_source_path(manifest.0, index),
                kind: onda_source_manifest_resolution_kind(manifest.0, index),
                specifier_utf8: onda_source_manifest_resolution_specifier(manifest.0, index),
                target_path_utf8: onda_source_manifest_resolution_target_path(manifest.0, index),
            })
            .collect::<Vec<_>>();
        assert_eq!(replay_sources.len(), 4);
        assert_eq!(replay_resolutions.len(), 3);
        let mut replay_manifest = std::ptr::null_mut();
        let replayed = onda_compile_source_graph(
            onda_source_manifest_path(manifest.0, 0),
            replay_sources.as_ptr(),
            replay_sources.len(),
            replay_resolutions.as_ptr(),
            replay_resolutions.len(),
            &options,
            &mut replay_manifest,
            &mut *diag,
        );
        assert!(
            !replayed.is_null(),
            "captured source graph did not replay: {}",
            diag_message(&diag)
        );
        let _replayed = ProgramHandle(replayed);
        let replay_manifest = SourceManifestHandle(replay_manifest);
        assert_eq!(onda_source_manifest_document_count(replay_manifest.0), 4);
        assert_eq!(onda_source_manifest_resolution_count(replay_manifest.0), 3);
        assert!(manifest_watch_paths(replay_manifest.0).is_empty());

        fs::write(&dependency, "this is not valid onda\n").expect("break dependency");
        let mut failed_manifest = std::ptr::null_mut();
        let failed = onda_compile_file(path.as_ptr(), &options, &mut failed_manifest, &mut *diag);
        assert!(failed.is_null(), "broken dependency unexpectedly compiled");
        let failed_manifest = SourceManifestHandle(failed_manifest);
        assert_eq!(
            manifest_paths(failed_manifest.0),
            vec![
                fs::canonicalize(&main).expect("canonical entry"),
                fs::canonicalize(&shared).expect("canonical include"),
                fs::canonicalize(&nested).expect("canonical nested import"),
                fs::canonicalize(&dependency).expect("canonical dependency"),
            ]
        );
        assert!(manifest_unresolved_paths(failed_manifest.0).is_empty());
        diag.clear();

        fs::write(&main, "import missing/module\n").expect("write missing import");
        let mut unresolved_manifest = std::ptr::null_mut();
        let unresolved = onda_compile_file(
            path.as_ptr(),
            &options,
            &mut unresolved_manifest,
            &mut *diag,
        );
        assert!(
            unresolved.is_null(),
            "missing dependency unexpectedly compiled"
        );
        let unresolved_manifest = SourceManifestHandle(unresolved_manifest);
        assert_eq!(
            manifest_unresolved_paths(unresolved_manifest.0),
            vec![
                dir.join("missing/module.onda"),
                dir.join("missing/module.on"),
            ]
        );
        assert_eq!(
            onda_source_manifest_unresolved_resolution_count(unresolved_manifest.0),
            1
        );
        assert_eq!(
            CStr::from_ptr(onda_source_manifest_unresolved_resolution_source_path(
                unresolved_manifest.0,
                0
            ))
            .to_str()
            .unwrap(),
            fs::canonicalize(&main)
                .expect("canonical entry")
                .to_str()
                .unwrap()
        );
        assert_eq!(
            onda_source_manifest_unresolved_resolution_kind(unresolved_manifest.0, 0),
            ONDA_SOURCE_REFERENCE_IMPORT
        );
        assert_eq!(
            CStr::from_ptr(onda_source_manifest_unresolved_resolution_specifier(
                unresolved_manifest.0,
                0
            ))
            .to_str()
            .unwrap(),
            "missing/module"
        );
        assert_eq!(
            onda_source_manifest_unresolved_resolution_candidate_count(unresolved_manifest.0, 0),
            2
        );
        assert_eq!(
            CStr::from_ptr(onda_source_manifest_unresolved_resolution_candidate_path(
                unresolved_manifest.0,
                0,
                0,
            ))
            .to_str()
            .unwrap(),
            dir.join("missing/module.onda").to_str().unwrap()
        );

        fs::remove_dir_all(&dir).ok();
    }
}

#[test]
fn c_file_compile_accepts_filesystem_projects_with_defaults_and_watch_paths() {
    unsafe {
        let dir = temp_source_dir("filesystem_project");
        let code_dir = dir.join("code");
        let asset_dir = dir.join("assets");
        fs::create_dir_all(&code_dir).expect("create project sources");
        fs::create_dir_all(&asset_dir).expect("create project assets");
        let project = dir.join("synth.ondaproject");
        let main = code_dir.join("main.onda");
        let voice = code_dir.join("voice.onda");
        let asset = asset_dir.join("sample.ondabuffer");

        fs::write(
            &project,
            r#"{
                "entry": "code/main.onda",
                "buffers": {"samples": {"file": "assets/sample.ondabuffer"}}
            }"#,
        )
        .expect("write project manifest");
        fs::write(
            &main,
            "import voice\nouts { out1 }\nbuffers { samples: buffer<f32> }\nsample { out1 = samples[0] * VOICE }\n",
        )
        .expect("write project entry");
        fs::write(&voice, "const VOICE = 1.0\n").expect("write project dependency");

        let samples = [0.75_f32];
        let mut diag = empty_diag();
        let encoded_bytes = onda_buffer_asset_encode(
            ONDA_PRIMITIVE_F32,
            1,
            1,
            48_000.0,
            samples.as_ptr().cast(),
            std::mem::size_of_val(&samples),
            std::ptr::null_mut(),
            0,
            &mut *diag,
        );
        assert!(
            encoded_bytes > 0,
            "asset sizing failed: {}",
            diag_message(&diag)
        );
        let mut encoded = vec![0_u8; encoded_bytes as usize];
        assert_eq!(
            onda_buffer_asset_encode(
                ONDA_PRIMITIVE_F32,
                1,
                1,
                48_000.0,
                samples.as_ptr().cast(),
                std::mem::size_of_val(&samples),
                encoded.as_mut_ptr().cast(),
                encoded.len(),
                &mut *diag,
            ),
            encoded_bytes
        );
        fs::write(&asset, encoded).expect("write project asset");

        let project_path = CString::new(project.to_str().expect("UTF-8 project path")).unwrap();
        let options = onda_compile_options_t {
            fast_math: 0,
            sample_rate: 48_000.0,
            block_size: 4,
        };
        let mut manifest = std::ptr::null_mut();
        let program = onda_compile_file(project_path.as_ptr(), &options, &mut manifest, &mut *diag);
        assert!(
            !program.is_null(),
            "filesystem project compile failed: {}",
            diag_message(&diag)
        );
        let program = ProgramHandle(program);
        let manifest = SourceManifestHandle(manifest);
        assert_eq!(
            manifest_paths(manifest.0),
            vec![
                fs::canonicalize(&main).expect("canonical project entry"),
                fs::canonicalize(&voice).expect("canonical project dependency"),
            ]
        );
        let mut watch_paths = manifest_watch_paths(manifest.0);
        watch_paths.sort();
        let mut expected_watch_paths = vec![
            fs::canonicalize(&project).expect("canonical project manifest"),
            fs::canonicalize(&main).expect("canonical project entry"),
            fs::canonicalize(&voice).expect("canonical project dependency"),
            fs::canonicalize(&asset).expect("canonical project asset"),
        ];
        expected_watch_paths.sort();
        assert_eq!(watch_paths, expected_watch_paths);

        let instance = onda_instance_create(program.0, 0, 1, &mut *diag);
        assert!(
            !instance.is_null(),
            "project instance creation failed: {}",
            diag_message(&diag)
        );
        let instance = InstanceHandle(instance);
        let mut output = [0.0_f32; 4];
        assert_eq!(
            onda_bind_output(
                instance.0,
                0,
                output.as_mut_ptr().cast(),
                std::mem::size_of_val(&output) as i32,
            ),
            0
        );
        assert_eq!(onda_process_checked(instance.0, output.len() as i32), 0);
        assert_eq!(output, [0.75; 4]);

        drop(instance);
        drop(program);
        drop(manifest);
        diag.clear();

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;

            let asset_target = asset_dir.join("sample-target.ondabuffer");
            fs::rename(&asset, &asset_target).expect("move asset behind a symlink");
            symlink(&asset_target, &asset).expect("create asset symlink");
            let mut failed_manifest = std::ptr::null_mut();
            let failed = onda_compile_file(
                project_path.as_ptr(),
                &options,
                &mut failed_manifest,
                &mut *diag,
            );
            assert!(
                failed.is_null(),
                "symlink project asset unexpectedly compiled"
            );
            let failed_manifest = SourceManifestHandle(failed_manifest);
            let failed_watch_paths = manifest_watch_paths(failed_manifest.0);
            assert!(failed_watch_paths.contains(&fs::canonicalize(&project).unwrap()));
            assert!(failed_watch_paths.contains(&fs::canonicalize(&main).unwrap()));
            assert!(failed_watch_paths.contains(&fs::canonicalize(&voice).unwrap()));
            assert!(failed_watch_paths.contains(&asset));

            drop(failed_manifest);
            fs::remove_file(&asset).expect("remove asset symlink");
            fs::rename(&asset_target, &asset).expect("restore regular asset");
            diag.clear();
        }

        fs::write(
            &project,
            r#"{
                "entry": "code/missing.onda",
                "buffers": {"samples": {"file": "assets/sample.ondabuffer"}}
            }"#,
        )
        .expect("point project at a missing entry");
        let mut failed_manifest = std::ptr::null_mut();
        let failed = onda_compile_file(
            project_path.as_ptr(),
            &options,
            &mut failed_manifest,
            &mut *diag,
        );
        assert!(
            failed.is_null(),
            "missing project entry unexpectedly compiled"
        );
        let failed_manifest = SourceManifestHandle(failed_manifest);
        let failed_watch_paths = manifest_watch_paths(failed_manifest.0);
        assert!(failed_watch_paths.contains(&fs::canonicalize(&project).unwrap()));
        assert!(failed_watch_paths.contains(&code_dir.join("missing.onda")));
        assert!(failed_watch_paths.contains(&fs::canonicalize(&asset).unwrap()));

        fs::remove_dir_all(dir).ok();
    }
}

#[test]
fn c_project_compile_replays_an_exact_in_memory_source_graph() {
    unsafe {
        let entry_path = CString::new("C:/saved/project/main.onda").unwrap();
        let dependency_path = CString::new("/original/shared/filter.onda").unwrap();
        let unused_empty_path = CString::new("unused-empty.onda").unwrap();
        let specifier = CString::new("/absolute/filter.onda").unwrap();
        let entry_source =
            "include \"/absolute/filter.onda\"\nouts 1\nsample:\n  out1 = FILTER_VALUE\n";
        let dependency_source = "const FILTER_VALUE = 0.375\n";
        let entry_source_c = CString::new(entry_source).unwrap();
        let dependency_source_c = CString::new(dependency_source).unwrap();
        let sources = [
            onda_source_graph_document_t {
                path_utf8: entry_path.as_ptr(),
                source_utf8: entry_source_c.as_ptr(),
                source_bytes: entry_source.len(),
            },
            onda_source_graph_document_t {
                path_utf8: dependency_path.as_ptr(),
                source_utf8: dependency_source_c.as_ptr(),
                source_bytes: dependency_source.len(),
            },
            onda_source_graph_document_t {
                path_utf8: unused_empty_path.as_ptr(),
                source_utf8: std::ptr::null(),
                source_bytes: 0,
            },
        ];
        let resolutions = [onda_source_graph_resolution_t {
            source_path_utf8: entry_path.as_ptr(),
            kind: ONDA_SOURCE_REFERENCE_INCLUDE,
            specifier_utf8: specifier.as_ptr(),
            target_path_utf8: dependency_path.as_ptr(),
        }];
        let options = onda_compile_options_t {
            fast_math: 0,
            sample_rate: 48_000.0,
            block_size: 64,
        };
        let mut diag = empty_diag();
        let mut manifest = std::ptr::null_mut();
        let program = onda_compile_source_graph(
            entry_path.as_ptr(),
            sources.as_ptr(),
            sources.len(),
            resolutions.as_ptr(),
            resolutions.len(),
            &options,
            &mut manifest,
            &mut *diag,
        );
        assert!(
            !program.is_null(),
            "project compile failed: {}",
            diag_message(&diag)
        );
        let _program = ProgramHandle(program);
        let manifest = SourceManifestHandle(manifest);
        assert_eq!(onda_source_manifest_document_count(manifest.0), 2);
        assert_eq!(onda_source_manifest_resolution_count(manifest.0), 1);
        assert_eq!(
            CStr::from_ptr(onda_source_manifest_resolution_source_path(manifest.0, 0))
                .to_str()
                .unwrap(),
            entry_path.to_str().unwrap()
        );
        assert_eq!(
            onda_source_manifest_resolution_kind(manifest.0, 0),
            ONDA_SOURCE_REFERENCE_INCLUDE
        );
        assert_eq!(
            CStr::from_ptr(onda_source_manifest_resolution_specifier(manifest.0, 0))
                .to_str()
                .unwrap(),
            specifier.to_str().unwrap()
        );
        assert_eq!(
            CStr::from_ptr(onda_source_manifest_resolution_target_path(manifest.0, 0))
                .to_str()
                .unwrap(),
            dependency_path.to_str().unwrap()
        );
        let mut source_bytes = 0;
        let source_ptr = onda_source_manifest_document_contents(manifest.0, 0, &mut source_bytes);
        assert_eq!(
            std::str::from_utf8(std::slice::from_raw_parts(
                source_ptr.cast::<u8>(),
                source_bytes
            ))
            .unwrap(),
            entry_source
        );
    }
}

#[test]
fn c_project_image_and_typed_asset_api_round_trip() {
    unsafe {
        let dir = std::env::temp_dir().join(format!(
            "onda_c_project_image_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        fs::create_dir_all(&dir).expect("create project source directory");
        let entry = dir.join("main.onda");
        let dependency = dir.join("voice.onda");
        fs::write(
            &entry,
            concat!(
                "include \"./voice.onda\"\n",
                "buffers:\n  sequence: buffer<i64>\n",
                "sample:\n  out1 = LEVEL\n",
            ),
        )
        .expect("write project entry");
        fs::write(&dependency, "const LEVEL = 0.25\n").expect("write project dependency");

        let entry_c = CString::new(entry.to_string_lossy().as_bytes()).unwrap();
        let root_c = CString::new(dir.to_string_lossy().as_bytes()).unwrap();
        let options = onda_compile_options_t {
            fast_math: 0,
            sample_rate: 48_000.0,
            block_size: 64,
        };
        let mut diag = empty_diag();
        let mut manifest = std::ptr::null_mut();
        let program = onda_compile_file(entry_c.as_ptr(), &options, &mut manifest, &mut *diag);
        assert!(
            !program.is_null(),
            "compile failed: {}",
            diag_message(&diag)
        );
        let _program = ProgramHandle(program);
        let manifest = SourceManifestHandle(manifest);

        assert_eq!(onda_project_image_format_version(), 1);
        assert_eq!(onda_buffer_asset_format_version(), 1);
        assert!(CStr::from_ptr(onda_current_stdlib_digest())
            .to_bytes()
            .starts_with(b"sha256:"));

        let samples = [-7_i64, i64::MAX];
        let required = onda_buffer_asset_encode(
            ONDA_PRIMITIVE_I64,
            2,
            1,
            48_000.0,
            samples.as_ptr().cast(),
            std::mem::size_of_val(&samples),
            std::ptr::null_mut(),
            0,
            &mut *diag,
        );
        assert!(required > 0, "asset sizing failed: {}", diag_message(&diag));
        let mut asset = vec![0_u8; required as usize];
        assert_eq!(
            onda_buffer_asset_encode(
                ONDA_PRIMITIVE_I64,
                2,
                1,
                48_000.0,
                samples.as_ptr().cast(),
                std::mem::size_of_val(&samples),
                asset.as_mut_ptr().cast(),
                asset.len(),
                &mut *diag,
            ),
            required
        );
        let mut decoded_info = onda_buffer_asset_info_t {
            element_type: -1,
            frames: 0,
            channels: 0,
            sample_rate: 0.0,
            sample_bytes: 0,
        };
        let mut decoded_samples = [0_i64; 2];
        assert_eq!(
            onda_buffer_asset_decode(
                asset.as_ptr().cast(),
                asset.len(),
                &mut decoded_info,
                decoded_samples.as_mut_ptr().cast(),
                std::mem::size_of_val(&decoded_samples),
                &mut *diag,
            ),
            std::mem::size_of_val(&decoded_samples) as i64
        );
        assert_eq!(decoded_info.element_type, ONDA_PRIMITIVE_I64);
        assert_eq!(decoded_samples, samples);

        let buffer_name = CString::new("sequence").unwrap();
        let binding = onda_project_buffer_asset_t {
            name_utf8: buffer_name.as_ptr(),
            ondabuffer_bytes: asset.as_ptr().cast(),
            ondabuffer_byte_count: asset.len(),
        };
        let image = onda_project_image_capture(
            entry_c.as_ptr(),
            root_c.as_ptr(),
            manifest.0,
            &binding,
            1,
            &mut *diag,
        );
        assert!(!image.is_null(), "capture failed: {}", diag_message(&diag));
        let image = ProjectImageHandle(image);
        let digest = CStr::from_ptr(onda_project_image_content_digest(image.0))
            .to_string_lossy()
            .into_owned();
        assert!(digest.starts_with("sha256:"));

        let image_bytes =
            onda_project_image_serialize(image.0, std::ptr::null_mut(), 0, &mut *diag);
        assert!(image_bytes > 0, "serialize failed: {}", diag_message(&diag));
        let mut serialized = vec![0_u8; image_bytes as usize];
        assert_eq!(
            onda_project_image_serialize(
                image.0,
                serialized.as_mut_ptr().cast(),
                serialized.len(),
                &mut *diag,
            ),
            image_bytes
        );
        let restored = onda_project_image_deserialize(
            serialized.as_ptr().cast(),
            serialized.len(),
            &mut *diag,
        );
        assert!(
            !restored.is_null(),
            "restore failed: {}",
            diag_message(&diag)
        );
        let restored = ProjectImageHandle(restored);
        assert_eq!(
            CStr::from_ptr(onda_project_image_content_digest(restored.0)).to_bytes(),
            digest.as_bytes()
        );
        assert_eq!(
            CStr::from_ptr(onda_project_image_entry(restored.0)).to_bytes(),
            b"main.onda"
        );
        assert_eq!(
            CStr::from_ptr(onda_project_image_stdlib_digest(restored.0)).to_bytes(),
            CStr::from_ptr(onda_current_stdlib_digest()).to_bytes()
        );
        assert_eq!(onda_project_image_document_count(restored.0), 2);
        assert_eq!(
            CStr::from_ptr(onda_project_image_document_path(restored.0, 0)).to_bytes(),
            b"main.onda"
        );
        let mut document_bytes = 0;
        let document = onda_project_image_document_contents(restored.0, 0, &mut document_bytes);
        assert_eq!(
            std::slice::from_raw_parts(document.cast::<u8>(), document_bytes),
            concat!(
                "include \"voice.onda\"\n",
                "buffers:\n  sequence: buffer<i64>\n",
                "sample:\n  out1 = LEVEL\n",
            )
            .as_bytes()
        );
        assert_eq!(onda_project_image_resolution_count(restored.0), 1);
        assert_eq!(
            onda_project_image_resolution_kind(restored.0, 0),
            ONDA_SOURCE_REFERENCE_INCLUDE
        );
        assert_eq!(onda_project_image_buffer_count(restored.0), 1);
        assert_eq!(
            CStr::from_ptr(onda_project_image_buffer_name(restored.0, 0)).to_bytes(),
            b"sequence"
        );
        assert!(
            CStr::from_ptr(onda_project_image_buffer_asset_id(restored.0, 0))
                .to_bytes()
                .starts_with(b"sha256:")
        );
        assert_eq!(
            onda_project_image_buffer_element_type(restored.0, 0),
            ONDA_PRIMITIVE_I64
        );
        assert_eq!(onda_project_image_buffer_frames(restored.0, 0), 2);
        assert_eq!(onda_project_image_buffer_channels(restored.0, 0), 1);
        assert_eq!(
            onda_project_image_buffer_sample_rate(restored.0, 0),
            48_000.0
        );
        let replayed = onda_project_image_compile(restored.0, &options, &mut *diag);
        assert!(
            !replayed.is_null(),
            "replay failed: {}",
            diag_message(&diag)
        );
        let _replayed = ProgramHandle(replayed);

        let plan = onda_project_image_materialize(restored.0, &mut *diag);
        assert!(
            !plan.is_null(),
            "materialize failed: {}",
            diag_message(&diag)
        );
        let plan = ProjectPlanHandle(plan);
        let paths = (0..onda_project_materialization_file_count(plan.0))
            .map(|index| {
                CStr::from_ptr(onda_project_materialization_file_path(plan.0, index))
                    .to_string_lossy()
                    .into_owned()
            })
            .collect::<Vec<_>>();
        assert!(paths.iter().any(|path| path == "code/main.onda"));
        assert!(paths.iter().any(|path| path == "code/voice.onda"));
        assert!(paths.iter().any(|path| path == "project.ondaproject"));
        assert!(paths.iter().any(|path| path.ends_with(".ondabuffer")));

        fs::remove_dir_all(dir).ok();
    }
}

#[test]
fn c_project_file_loader_accepts_an_explicit_manifest_selection() {
    let paths = [
        CString::new("first.ondaproject").unwrap(),
        CString::new("second.ondaproject").unwrap(),
        CString::new("first.onda").unwrap(),
        CString::new("second.onda").unwrap(),
    ];
    let contents = [
        br#"{"entry":"first.onda"}"#.to_vec(),
        br#"{"entry":"second.onda"}"#.to_vec(),
        b"outs 1\nsample:\n  out1 = 1.0\n".to_vec(),
        b"outs 1\nsample:\n  out1 = 2.0\n".to_vec(),
    ];
    let files = paths
        .iter()
        .zip(&contents)
        .map(|(path, contents)| onda_project_file_t {
            path_utf8: path.as_ptr(),
            bytes: contents.as_ptr().cast(),
            byte_count: contents.len(),
        })
        .collect::<Vec<_>>();
    let selected = CString::new("second.ondaproject").unwrap();
    let mut diag = empty_diag();

    let image = unsafe {
        onda_project_image_load_files(files.as_ptr(), files.len(), selected.as_ptr(), &mut *diag)
    };
    assert!(!image.is_null(), "load failed: {}", diag_message(&diag));
    let image = ProjectImageHandle(image);
    assert_eq!(
        unsafe { CStr::from_ptr(onda_project_image_entry(image.0)) }.to_bytes(),
        b"second.onda"
    );
}

#[test]
fn project_instances_share_immutable_defaults_and_allow_host_overrides() {
    unsafe {
        let paths = [
            CString::new("project.ondaproject").unwrap(),
            CString::new("main.onda").unwrap(),
        ];
        let contents = [
            br#"{
                "entry": "main.onda",
                "buffers": {
                    "samples": {
                        "inline": {
                            "element": "f32",
                            "channels": 1,
                            "sample_rate": 48000,
                            "values": [0.75]
                        }
                    }
                }
            }"#
            .to_vec(),
            br#"
outs { out1 }
buffers { samples: buffer<f32> }
sample { out1 = samples[0] }
"#
            .to_vec(),
        ];
        let files = paths
            .iter()
            .zip(&contents)
            .map(|(path, contents)| onda_project_file_t {
                path_utf8: path.as_ptr(),
                bytes: contents.as_ptr().cast(),
                byte_count: contents.len(),
            })
            .collect::<Vec<_>>();
        let mut diag = empty_diag();
        let image = onda_project_image_load_files(
            files.as_ptr(),
            files.len(),
            paths[0].as_ptr(),
            &mut *diag,
        );
        assert!(!image.is_null(), "load failed: {}", diag_message(&diag));

        let options = onda_compile_options_t {
            fast_math: 0,
            sample_rate: 48_000.0,
            block_size: 4,
        };
        let program = onda_project_image_compile(image, &options, &mut *diag);
        assert!(
            !program.is_null(),
            "project compile failed: {}",
            diag_message(&diag)
        );
        onda_project_image_destroy(image);

        let instance = onda_instance_create(program, 0, 1, &mut *diag);
        assert!(
            !instance.is_null(),
            "instance create failed: {}",
            diag_message(&diag)
        );
        onda_program_destroy(program);
        let instance = InstanceHandle(instance);

        let mut output = [0.0_f32; 4];
        assert_eq!(
            onda_bind_output(
                instance.0,
                0,
                output.as_mut_ptr().cast(),
                std::mem::size_of_val(&output) as i32,
            ),
            0
        );
        assert_eq!(onda_process_checked(instance.0, output.len() as i32), 0);
        assert_eq!(output, [0.75; 4]);

        let mut host_samples = [0.25_f32];
        assert_eq!(
            onda_bind_buffer(
                instance.0,
                0,
                host_samples.as_mut_ptr().cast(),
                1,
                1,
                48_000.0,
                ONDA_PRIMITIVE_F32,
            ),
            0
        );
        assert_eq!(onda_process_checked(instance.0, output.len() as i32), 0);
        assert_eq!(output, [0.25; 4]);

        assert_eq!(onda_reset_buffer_to_project_default(instance.0, 0), 0);
        assert_eq!(onda_process_checked(instance.0, output.len() as i32), 0);
        assert_eq!(output, [0.75; 4]);
    }
}

#[test]
fn project_compilation_rejects_writes_to_immutable_assets() {
    unsafe {
        let paths = [
            CString::new("project.ondaproject").unwrap(),
            CString::new("main.onda").unwrap(),
        ];
        let contents = [
            br#"{
                "entry": "main.onda",
                "buffers": {
                    "samples": {
                        "inline": {
                            "element": "f32",
                            "channels": 1,
                            "sample_rate": 48000,
                            "values": [0.75]
                        }
                    }
                }
            }"#
            .to_vec(),
            br#"
buffers { samples: buffer<f32> }
sample { samples[0] = 0.0 }
"#
            .to_vec(),
        ];
        let files = paths
            .iter()
            .zip(&contents)
            .map(|(path, contents)| onda_project_file_t {
                path_utf8: path.as_ptr(),
                bytes: contents.as_ptr().cast(),
                byte_count: contents.len(),
            })
            .collect::<Vec<_>>();
        let mut diag = empty_diag();
        let image = onda_project_image_load_files(
            files.as_ptr(),
            files.len(),
            paths[0].as_ptr(),
            &mut *diag,
        );
        assert!(!image.is_null(), "load failed: {}", diag_message(&diag));
        let image = ProjectImageHandle(image);
        let options = onda_compile_options_t {
            fast_math: 0,
            sample_rate: 48_000.0,
            block_size: 4,
        };
        let program = onda_project_image_compile(image.0, &options, &mut *diag);
        assert!(program.is_null());
        let message = diag_message(&diag);
        assert!(message.contains("immutable"), "unexpected error: {message}");
        assert!(message.contains("may write"), "unexpected error: {message}");
    }
}

#[test]
fn c_api_rewrites_only_parsed_source_references() {
    unsafe {
        let path = CString::new("saved/main.onda").unwrap();
        let source = concat!(
            "# include \"unchanged.onda\"\r\n",
            "sample:\r\n",
            "  out1 = 0.0\r\n",
            "include \"old/shared.onda\" # keep\r\n",
            "import old/module\r\n",
            "import std/math\r\n",
        );
        let include = CString::new("old/shared.onda").unwrap();
        let include_replacement = CString::new("external/shared.onda").unwrap();
        let import = CString::new("old/module").unwrap();
        let import_replacement = CString::new("sources/module").unwrap();
        let rewrites = [
            onda_source_rewrite_t {
                kind: ONDA_SOURCE_REFERENCE_INCLUDE,
                specifier_utf8: include.as_ptr(),
                replacement_utf8: include_replacement.as_ptr(),
            },
            onda_source_rewrite_t {
                kind: ONDA_SOURCE_REFERENCE_IMPORT,
                specifier_utf8: import.as_ptr(),
                replacement_utf8: import_replacement.as_ptr(),
            },
        ];
        let mut diag = empty_diag();
        let required = onda_rewrite_source_references(
            path.as_ptr(),
            source.as_ptr().cast::<c_char>(),
            source.len(),
            rewrites.as_ptr(),
            rewrites.len(),
            std::ptr::null_mut(),
            0,
            &mut *diag,
        );
        assert!(required > 0, "rewrite failed: {}", diag_message(&diag));
        let mut output = vec![0_u8; required as usize];
        assert_eq!(
            onda_rewrite_source_references(
                path.as_ptr(),
                source.as_ptr().cast::<c_char>(),
                source.len(),
                rewrites.as_ptr(),
                rewrites.len(),
                output.as_mut_ptr().cast::<c_char>(),
                required,
                &mut *diag,
            ),
            required
        );
        assert_eq!(
            std::str::from_utf8(&output).unwrap(),
            concat!(
                "# include \"unchanged.onda\"\r\n",
                "sample:\r\n",
                "  out1 = 0.0\r\n",
                "include \"external/shared.onda\" # keep\r\n",
                "import sources/module\r\n",
                "import std/math\r\n",
            )
        );
    }
}

#[test]
fn c_api_compile_reports_diagnostic_ranges() {
    unsafe {
        let src = CString::new(
            r#"
const BAD = false
outs:
  out1
sample:
  a = [0.0, 0.0]
  a[BAD:] = 0.5
  out1 = a[0]
"#,
        )
        .expect("source contains no NUL bytes");
        let options = onda_compile_options_t {
            fast_math: 0,
            sample_rate: 48_000.0,
            block_size: 512,
        };
        let mut diag = empty_diag();
        let program = onda_compile(src.as_ptr(), &options, &mut *diag);
        assert!(program.is_null(), "compile unexpectedly succeeded");
        assert_eq!((diag.line, diag.column), (7, 5));
        assert_eq!(diag.end_line, 7);
        assert_eq!(diag.end_column, 8);
    }
}

#[test]
fn c_api_event_metadata_queries_work() {
    unsafe {
        let program = compile_program(
            r#"
outs { out1 }
events {
  note_on(note: i32 = 60, vel: i32, accent: bool = true) {
    amp = f32(vel) / 127.0
  }
  set_curve(values: f32[2] = [0.25, 0.75]) {
    amp = values[0] + values[1]
  }
}
init { amp = 0.0 }
sample { out1 = amp }
"#,
        );

        assert_eq!(onda_event_count(program.0), 2);
        assert_eq!(onda_event_param_count(program.0, 0), 3);
        assert_eq!(onda_event_param_count(program.0, 1), 1);
        let name0 = CStr::from_ptr(onda_event_name(program.0, 0))
            .to_string_lossy()
            .into_owned();
        let name1 = CStr::from_ptr(onda_event_name(program.0, 1))
            .to_string_lossy()
            .into_owned();
        assert_eq!(name0, "note_on");
        assert_eq!(name1, "set_curve");

        let note_on = CString::new("note_on").expect("valid cstr");
        let set_curve = CString::new("set_curve").expect("valid cstr");
        assert_eq!(onda_event_index(program.0, note_on.as_ptr()), 0);
        assert_eq!(onda_event_index(program.0, set_curve.as_ptr()), 1);
        assert_eq!(onda_event_payload_bytes(program.0, 0), 9);
        assert_eq!(onda_event_payload_bytes(program.0, 1), 8);

        let note_name = CStr::from_ptr(onda_event_param_name(program.0, 0, 0))
            .to_string_lossy()
            .into_owned();
        let vel_name = CStr::from_ptr(onda_event_param_name(program.0, 0, 1))
            .to_string_lossy()
            .into_owned();
        let accent_name = CStr::from_ptr(onda_event_param_name(program.0, 0, 2))
            .to_string_lossy()
            .into_owned();
        assert_eq!(note_name, "note");
        assert_eq!(vel_name, "vel");
        assert_eq!(accent_name, "accent");

        assert_eq!(onda_event_param_elem_type(program.0, 0, 0), 2);
        assert_eq!(onda_event_param_array_len(program.0, 0, 0), 1);
        assert_eq!(onda_event_param_is_slice(program.0, 0, 0), 0);
        assert_eq!(onda_event_param_offset_bytes(program.0, 0, 0), 0);
        assert_eq!(onda_event_param_has_default(program.0, 0, 0), 1);
        assert_eq!(
            onda_event_param_default_bytes(program.0, 0, 0, std::ptr::null_mut(), 0),
            4
        );
        let mut note_default = [0_u8; 4];
        assert_eq!(
            onda_event_param_default_bytes(
                program.0,
                0,
                0,
                note_default.as_mut_ptr().cast::<c_void>(),
                note_default.len() as i32,
            ),
            4
        );
        assert_eq!(i32::from_ne_bytes(note_default), 60);

        assert_eq!(onda_event_param_has_default(program.0, 0, 1), 0);
        assert_eq!(
            onda_event_param_default_bytes(program.0, 0, 1, std::ptr::null_mut(), 0),
            0
        );

        assert_eq!(onda_event_param_elem_type(program.0, 0, 2), 4);
        assert_eq!(onda_event_param_offset_bytes(program.0, 0, 2), 8);
        let mut accent_default = [0_u8; 1];
        assert_eq!(
            onda_event_param_default_bytes(
                program.0,
                0,
                2,
                accent_default.as_mut_ptr().cast::<c_void>(),
                accent_default.len() as i32,
            ),
            1
        );
        assert_eq!(accent_default[0], 1);

        assert_eq!(onda_event_param_elem_type(program.0, 1, 0), 0);
        assert_eq!(onda_event_param_array_len(program.0, 1, 0), 2);
        assert_eq!(onda_event_param_is_slice(program.0, 1, 0), 0);
        assert_eq!(onda_event_param_offset_bytes(program.0, 1, 0), 0);
        assert_eq!(onda_event_param_has_default(program.0, 1, 0), 1);
        assert_eq!(
            onda_event_param_default_bytes(program.0, 1, 0, std::ptr::null_mut(), 0),
            8
        );
        let mut curve_default = [0_u8; 8];
        assert_eq!(
            onda_event_param_default_bytes(
                program.0,
                1,
                0,
                curve_default.as_mut_ptr().cast::<c_void>(),
                curve_default.len() as i32,
            ),
            8
        );
        assert_eq!(
            f32::from_ne_bytes(curve_default[0..4].try_into().unwrap()),
            0.25
        );
        assert_eq!(
            f32::from_ne_bytes(curve_default[4..8].try_into().unwrap()),
            0.75
        );
    }
}

#[test]
fn c_api_param_default_bytes_preserve_declared_types_and_arrays() {
    unsafe {
        let program = compile_program(
            r#"
params {
  gain: f32 = 0.25
  ratio: f64 = 0.5
  mode: i32 = 7
  wide: i64 = 1099511627776
  gate: bool = true
  curve: f32[2] = [0.25, 0.75]
}
outs { out1 }
sample { out1 = gain }
"#,
        );

        assert_eq!(onda_param_count(program.0), 6);

        assert_eq!(onda_param_type_bytes(program.0, 0), 4);
        assert_eq!(
            onda_param_default_bytes(program.0, 0, std::ptr::null_mut(), 0),
            4
        );
        let mut gain = [0_u8; 4];
        assert_eq!(
            onda_param_default_bytes(
                program.0,
                0,
                gain.as_mut_ptr().cast::<c_void>(),
                gain.len() as i32,
            ),
            4
        );
        assert_eq!(f32::from_ne_bytes(gain), 0.25);

        let mut ratio = [0_u8; 8];
        assert_eq!(
            onda_param_default_bytes(
                program.0,
                1,
                ratio.as_mut_ptr().cast::<c_void>(),
                ratio.len() as i32,
            ),
            8
        );
        assert_eq!(f64::from_ne_bytes(ratio), 0.5);

        let mut mode = [0_u8; 4];
        assert_eq!(
            onda_param_default_bytes(
                program.0,
                2,
                mode.as_mut_ptr().cast::<c_void>(),
                mode.len() as i32,
            ),
            4
        );
        assert_eq!(i32::from_ne_bytes(mode), 7);

        let mut wide = [0_u8; 8];
        assert_eq!(
            onda_param_default_bytes(
                program.0,
                3,
                wide.as_mut_ptr().cast::<c_void>(),
                wide.len() as i32,
            ),
            8
        );
        assert_eq!(i64::from_ne_bytes(wide), 1_099_511_627_776);

        let mut gate = [0_u8; 1];
        assert_eq!(
            onda_param_default_bytes(
                program.0,
                4,
                gate.as_mut_ptr().cast::<c_void>(),
                gate.len() as i32,
            ),
            1
        );
        assert_eq!(gate[0], 1);

        assert_eq!(onda_param_array_len(program.0, 5), 2);
        assert_eq!(
            onda_param_default_bytes(program.0, 5, std::ptr::null_mut(), 0),
            8
        );
        let mut curve = [0_u8; 8];
        assert_eq!(
            onda_param_default_bytes(
                program.0,
                5,
                curve.as_mut_ptr().cast::<c_void>(),
                curve.len() as i32,
            ),
            8
        );
        assert_eq!(f32::from_ne_bytes(curve[0..4].try_into().unwrap()), 0.25);
        assert_eq!(f32::from_ne_bytes(curve[4..8].try_into().unwrap()), 0.75);

        let mut too_small = [0_u8; 1];
        assert_eq!(
            onda_param_default_bytes(
                program.0,
                5,
                too_small.as_mut_ptr().cast::<c_void>(),
                too_small.len() as i32,
            ),
            8
        );
        assert_eq!(too_small, [0]);
        assert_eq!(
            onda_param_default_bytes(program.0, -1, std::ptr::null_mut(), 0),
            -1
        );
    }
}

#[test]
fn c_api_control_outputs_metadata_and_readback_work() {
    unsafe {
        let frames = 512_i32;
        let program = compile_program(
            r#"
kouts {
  meter: f32
  flags: bool[2]
}

block {
  meter = 3.5
  flags[0] = true
  flags[1] = false
}
"#,
        );

        assert_eq!(onda_output_count(program.0), 0);
        assert_eq!(onda_control_output_count(program.0), 2);

        let meter_name = CStr::from_ptr(onda_control_output_name(program.0, 0))
            .to_string_lossy()
            .into_owned();
        let flags_name = CStr::from_ptr(onda_control_output_name(program.0, 1))
            .to_string_lossy()
            .into_owned();
        assert_eq!(meter_name, "meter");
        assert_eq!(flags_name, "flags");

        let flags_key = CString::new("flags").expect("name contains no NUL bytes");
        assert_eq!(onda_control_output_index(program.0, flags_key.as_ptr()), 1);
        assert_eq!(onda_control_output_elem_type(program.0, 0), 0);
        assert_eq!(onda_control_output_type_bytes(program.0, 0), 4);
        assert_eq!(onda_control_output_array_len(program.0, 0), 1);
        assert_eq!(onda_control_output_elem_type(program.0, 1), 4);
        assert_eq!(onda_control_output_type_bytes(program.0, 1), 2);
        assert_eq!(onda_control_output_array_len(program.0, 1), 2);
        assert!(onda_control_output_slot_offset(program.0, 1) >= 0);
        assert!(onda_control_output_byte_offset(program.0, 1) >= 0);

        let mut diag = empty_diag();
        let instance = onda_instance_create(program.0, 0, 0, &mut *diag);
        assert!(
            !instance.is_null(),
            "instance create failed: {}",
            diag_message(&diag)
        );
        let instance = InstanceHandle(instance);

        assert_eq!(onda_process_checked(instance.0, frames), 0);

        let mut meter = [0_u8; 4];
        assert_eq!(
            onda_control_output_read_bytes(
                instance.0,
                0,
                meter.as_mut_ptr().cast::<c_void>(),
                meter.len() as i32,
            ),
            4
        );
        assert!((f32::from_ne_bytes(meter) - 3.5).abs() < 1e-6);

        let mut flags = [0_u8; 2];
        assert_eq!(
            onda_control_output_read_bytes(
                instance.0,
                1,
                flags.as_mut_ptr().cast::<c_void>(),
                flags.len() as i32,
            ),
            2
        );
        assert_eq!(flags, [1, 0]);
    }
}

#[test]
fn c_api_trigger_event_by_index_validates_and_dispatches() {
    unsafe {
        let frames = 512_i32;
        let program = compile_program(
            r#"
outs { out1 }
events {
  set_amp(value: f32) {
    amp = value
  }
}
init { amp = 0.0 }
sample { out1 = amp }
"#,
        );

        let mut diag = empty_diag();
        let instance = onda_instance_create(program.0, 0, 1, &mut *diag);
        assert!(
            !instance.is_null(),
            "instance create failed: {}",
            diag_message(&diag)
        );
        let instance = InstanceHandle(instance);

        let mut out = vec![0.0_f32; frames as usize];
        assert_eq!(
            onda_bind_output(
                instance.0,
                0,
                out.as_mut_ptr().cast::<c_void>(),
                (out.len() * std::mem::size_of::<f32>()) as i32,
            ),
            0
        );

        assert_eq!(
            onda_trigger_event_by_index(instance.0, 99, std::ptr::null(), 0),
            0
        );
        assert_eq!(onda_process_checked(instance.0, frames), 0);
        for sample in &out {
            assert_eq!(*sample, 0.0);
        }

        assert_eq!(
            onda_trigger_event_by_index(instance.0, 0, std::ptr::null(), 0),
            -2
        );

        let payload = 0.625_f32.to_ne_bytes();
        assert_eq!(
            onda_trigger_event_by_index(
                instance.0,
                0,
                payload.as_ptr().cast::<c_void>(),
                payload.len() as i32,
            ),
            0
        );
        assert_eq!(onda_process_checked(instance.0, frames), 0);
        for sample in &out {
            assert!((*sample - 0.625).abs() < 1e-6);
        }
    }
}

#[test]
fn c_api_slice_events_report_dynamic_payload_and_dispatch() {
    unsafe {
        let frames = 512_i32;
        let program = compile_program(
            r#"
outs { out1 }
init { gate = 0.0 }
events {
  load(values: f32[]) {
    gate = values[0] + f32(values.len())
  }
}
sample { out1 = gate }
"#,
        );

        let load = CString::new("load").expect("valid cstr");
        let event_idx = onda_event_index(program.0, load.as_ptr());
        assert_eq!(event_idx, 0);
        assert_eq!(onda_event_payload_bytes(program.0, event_idx), -1);

        let mut diag = empty_diag();
        let instance = onda_instance_create(program.0, 0, 1, &mut *diag);
        assert!(
            !instance.is_null(),
            "instance create failed: {}",
            diag_message(&diag)
        );
        let instance = InstanceHandle(instance);

        let mut out = vec![0.0_f32; frames as usize];
        assert_eq!(
            onda_bind_output(
                instance.0,
                0,
                out.as_mut_ptr().cast::<c_void>(),
                (out.len() * std::mem::size_of::<f32>()) as i32,
            ),
            0
        );

        let bad_payload = 2_i32.to_ne_bytes();
        assert_eq!(
            onda_trigger_event_by_index(
                instance.0,
                event_idx,
                bad_payload.as_ptr().cast::<c_void>(),
                bad_payload.len() as i32,
            ),
            -2
        );

        let mut payload = Vec::new();
        payload.extend_from_slice(&(2_i32).to_ne_bytes());
        payload.extend_from_slice(&0.25_f32.to_ne_bytes());
        payload.extend_from_slice(&0.75_f32.to_ne_bytes());
        assert_eq!(
            onda_trigger_event_by_index(
                instance.0,
                event_idx,
                payload.as_ptr().cast::<c_void>(),
                payload.len() as i32,
            ),
            0
        );
        assert_eq!(onda_process_checked(instance.0, frames), 0);
        for sample in out {
            assert!((sample - 2.25).abs() < 1e-6);
        }
    }
}

#[test]
fn c_api_reset_restores_initial_state() {
    unsafe {
        let frames = 512_i32;
        let program = compile_program(
            r#"
outs { out1 }
events {
  set_amp(value: f32) {
    amp = value
    pinned = value + 1.0
  }
}
init {
  amp = 0.0
  pin pinned = 1.0
}
sample { out1 = amp + pinned }
"#,
        );

        let mut diag = empty_diag();
        let instance = onda_instance_create(program.0, 0, 1, &mut *diag);
        assert!(
            !instance.is_null(),
            "instance create failed: {}",
            diag_message(&diag)
        );
        let instance = InstanceHandle(instance);

        let mut out = vec![0.0_f32; frames as usize];
        assert_eq!(
            onda_bind_output(
                instance.0,
                0,
                out.as_mut_ptr().cast::<c_void>(),
                (out.len() * std::mem::size_of::<f32>()) as i32,
            ),
            0
        );

        let payload = 0.5_f32.to_ne_bytes();
        assert_eq!(
            onda_trigger_event_by_index(
                instance.0,
                0,
                payload.as_ptr().cast::<c_void>(),
                payload.len() as i32,
            ),
            0
        );
        assert_eq!(onda_process_checked(instance.0, frames), 0);
        for sample in &out {
            assert!((*sample - 2.0).abs() < 1e-6, "got {sample}");
        }

        assert_eq!(onda_reset(instance.0), 0);
        assert_eq!(onda_process_checked(instance.0, frames), 0);
        for sample in &out {
            assert!((*sample - 1.5).abs() < 1e-6, "got {sample}");
        }

        assert_eq!(onda_reset_all(instance.0), 0);
        assert_eq!(onda_process_checked(instance.0, frames), 0);
        for sample in &out {
            assert!((*sample - 1.0).abs() < 1e-6);
        }

        assert_eq!(
            onda_trigger_event_by_index(
                instance.0,
                0,
                payload.as_ptr().cast::<c_void>(),
                payload.len() as i32,
            ),
            0
        );
        assert_eq!(onda_init(instance.0), 0);
        assert_eq!(onda_process_checked(instance.0, frames), 0);
        for sample in &out {
            assert!((*sample - 1.5).abs() < 1e-6, "got {sample}");
        }

        let changed_payload = 0.25_f32.to_ne_bytes();
        assert_eq!(
            onda_trigger_event_by_index(
                instance.0,
                0,
                changed_payload.as_ptr().cast::<c_void>(),
                changed_payload.len() as i32,
            ),
            0
        );
        assert_eq!(onda_reset(instance.0), 0);
        assert_eq!(onda_process_checked(instance.0, frames), 0);
        for sample in &out {
            assert!((*sample - 1.25).abs() < 1e-6);
        }

        assert_eq!(onda_reset_all(instance.0), 0);
        assert_eq!(onda_process_checked(instance.0, frames), 0);
        for sample in &out {
            assert!((*sample - 1.5).abs() < 1e-6);
        }

        assert_eq!(onda_init_all(instance.0), 0);
        assert_eq!(onda_process_checked(instance.0, frames), 0);
        for sample in &out {
            assert!((*sample - 1.0).abs() < 1e-6);
        }

        assert_eq!(onda_init(std::ptr::null_mut()), -1);
        assert_eq!(onda_init_all(std::ptr::null_mut()), -1);
    }
}

#[test]
fn c_api_custom_allocator_instance_uses_allocator_and_frees_it() {
    unsafe {
        let frames = 512_i32;
        let program = compile_program(
            r#"
outs { out1 }
init { phase = 0.25 }
sample {
  phase = phase + 0.25
  out1 = phase
}
"#,
        );

        let mut stats = AllocStats::default();
        let allocator = onda_allocator_t {
            context: (&mut stats as *mut AllocStats).cast::<c_void>(),
            alloc: Some(test_alloc),
            free: Some(test_free),
        };
        let mut diag = empty_diag();
        let instance = onda_instance_create_with_allocator(program.0, 0, 1, &allocator, &mut *diag);
        assert!(
            !instance.is_null(),
            "instance create failed: {}",
            diag_message(&diag)
        );
        assert!(stats.allocs > 0);

        let mut out = vec![0.0_f32; frames as usize];
        assert_eq!(
            onda_bind_output(
                instance,
                0,
                out.as_mut_ptr().cast::<c_void>(),
                (out.len() * std::mem::size_of::<f32>()) as i32,
            ),
            0
        );
        assert_eq!(onda_validate_bindings(instance), 0);
        assert_eq!(onda_process_unchecked(instance), 0);
        assert!((out[0] - 0.5).abs() < 1e-6);

        onda_instance_destroy(instance);
        assert_eq!(stats.live, 0);
        assert_eq!(stats.allocs, stats.frees);
    }
}

#[test]
fn c_api_custom_allocator_allocates_on_creation_thread_and_frees_on_instance_owner_thread() {
    unsafe {
        let frames = 512_usize;
        let program = compile_program("sample:\n  out1 = 0.25\n");
        let stats = Box::new(ThreadBoundAllocStats::new());
        let allocator = onda_allocator_t {
            context: (stats.as_ref() as *const ThreadBoundAllocStats)
                .cast_mut()
                .cast::<c_void>(),
            alloc: Some(thread_bound_alloc),
            free: Some(thread_bound_free),
        };
        let mut diag = empty_diag();
        let instance = onda_instance_create_with_allocator(program.0, 0, 1, &allocator, &mut *diag);
        assert!(
            !instance.is_null(),
            "instance create failed: {}",
            diag_message(&diag)
        );
        assert!(stats.allocs.load(Ordering::Relaxed) > 0);

        let transferred = TransferredInstance(instance);
        let output = std::thread::spawn(move || {
            let instance = transferred.into_raw();
            let mut output = vec![0.0_f32; frames];
            assert_eq!(
                onda_bind_output(
                    instance,
                    0,
                    output.as_mut_ptr().cast::<c_void>(),
                    std::mem::size_of_val(output.as_slice()) as i32,
                ),
                0
            );
            assert_eq!(onda_process_checked(instance, frames as i32), 0);
            onda_instance_destroy(instance);
            output
        })
        .join()
        .expect("instance owner thread should complete");

        assert!(output.iter().all(|sample| *sample == 0.25));
        assert_eq!(stats.rejected_foreign_allocs.load(Ordering::Relaxed), 0);
        assert!(stats.foreign_frees.load(Ordering::Relaxed) > 0);
        assert_eq!(
            stats.allocs.load(Ordering::Relaxed),
            stats.frees.load(Ordering::Relaxed)
        );
    }
}

#[test]
fn c_api_state_manifest_and_snapshot_restore_work() {
    unsafe {
        let frames = 512_i32;
        let program = compile_program(
            r#"
outs { out1 }
init { phase = 0.0 }
sample {
  phase = phase + 1.0
  out1 = phase
}
"#,
        );

        assert_eq!(onda_state_count(program.0), 1);
        assert_eq!(
            CStr::from_ptr(onda_state_name(program.0, 0)).to_string_lossy(),
            "phase"
        );
        assert_eq!(
            CStr::from_ptr(onda_state_type(program.0, 0)).to_string_lossy(),
            "f32"
        );
        assert_eq!(onda_state_elem_type(program.0, 0), 0);
        assert_eq!(onda_state_array_len(program.0, 0), 1);
        assert_eq!(onda_state_type_bytes(program.0, 0), 4);
        assert_eq!(onda_state_byte_offset(program.0, 0), 0);
        assert!(onda_state_total_bytes(program.0) >= 4);

        let mut diag = empty_diag();
        let instance = onda_instance_create(program.0, 0, 1, &mut *diag);
        assert!(
            !instance.is_null(),
            "instance create failed: {}",
            diag_message(&diag)
        );
        let instance = InstanceHandle(instance);

        let mut out = vec![0.0_f32; frames as usize];
        assert_eq!(
            onda_bind_output(
                instance.0,
                0,
                out.as_mut_ptr().cast::<c_void>(),
                (out.len() * std::mem::size_of::<f32>()) as i32,
            ),
            0
        );

        assert_eq!(onda_process_checked(instance.0, frames), 0);
        assert_eq!(out[0], 1.0);
        assert_eq!(out[frames as usize - 1], 512.0);

        let state_bytes = onda_instance_state_bytes(instance.0);
        assert_eq!(state_bytes, onda_state_total_bytes(program.0));
        let mut snapshot = vec![0_u8; state_bytes as usize];
        assert_eq!(
            onda_instance_snapshot_state(
                instance.0,
                snapshot.as_mut_ptr().cast::<c_void>(),
                snapshot.len() as i32,
            ),
            state_bytes
        );

        assert_eq!(onda_process_checked(instance.0, frames), 0);
        assert_eq!(out[0], 513.0);
        assert_eq!(out[frames as usize - 1], 1024.0);

        assert_eq!(
            onda_instance_restore_state(
                instance.0,
                snapshot.as_ptr().cast::<c_void>(),
                snapshot.len() as i32,
            ),
            0
        );
        assert_eq!(onda_process_checked(instance.0, frames), 0);
        assert_eq!(out[0], 513.0);
        assert_eq!(out[frames as usize - 1], 1024.0);
    }
}

#[test]
fn c_api_parameter_unit_query_distinguishes_absence_from_invalid_index() {
    unsafe {
        let program = compile_program(
            r#"
params {
  cutoff = 440.0 {20, 20000, log, "Hz"}
  gain = 1.0 {0, 2, curve = -4}
}
outs { out1 }
sample { out1 = cutoff * gain }
"#,
        );

        assert_eq!(
            onda_param_unit_copy(program.0, 0, std::ptr::null_mut(), 0),
            3
        );
        let mut unit = [0 as c_char; 3];
        assert_eq!(
            onda_param_unit_copy(program.0, 0, unit.as_mut_ptr(), unit.len() as i32),
            3
        );
        assert_eq!(CStr::from_ptr(unit.as_ptr()).to_bytes(), b"Hz");
        assert_eq!(
            onda_param_unit_copy(program.0, 1, std::ptr::null_mut(), 0),
            0
        );
        assert_eq!(
            onda_param_unit_copy(program.0, 2, std::ptr::null_mut(), 0),
            -1
        );
        assert_eq!(onda_param_has_curve(program.0, 0), 0);
        assert!(onda_param_curve(program.0, 0).is_nan());
        assert_eq!(onda_param_has_curve(program.0, 1), 1);
        assert_eq!(onda_param_curve(program.0, 1), -4.0);
        assert_eq!(onda_param_has_curve(program.0, 2), -1);
        assert!(onda_param_curve(program.0, 2).is_nan());
    }
}

#[test]
fn c_api_parameter_conversions_support_boolean_thresholds() {
    unsafe {
        let program = compile_program(
            r#"
params {
  gate: bool = false
  flags: bool[1] = [false]
}
outs { out1 }
sample { out1 = 0.0 }
"#,
        );

        assert_eq!(onda_param_normalized_to_plain(program.0, 0, 0.49), 0.0);
        assert_eq!(onda_param_normalized_to_plain(program.0, 0, 0.5), 1.0);
        assert_eq!(onda_param_plain_to_normalized(program.0, 0, -1.0), 0.0);
        assert_eq!(onda_param_plain_to_normalized(program.0, 0, 0.5), 1.0);
        assert_eq!(onda_param_normalized_to_plain(program.0, 0, f64::NAN), 0.0);

        assert!(onda_param_normalized_to_plain(program.0, 1, 1.0).is_nan());
        assert!(onda_param_plain_to_normalized(program.0, 2, 1.0).is_nan());
    }
}

#[test]
fn c_api_compile_options_block_size_controls_runtime_block_size() {
    unsafe {
        let src = CString::new(
            r#"
outs { out1 }
sample { out1 = 0.25 }
"#,
        )
        .expect("source contains no NUL bytes");

        let options = onda_compile_options_t {
            fast_math: 0,
            sample_rate: 48_000.0,
            block_size: 128,
        };
        let mut diag = empty_diag();
        let program = onda_compile(src.as_ptr(), &options, &mut *diag);
        assert!(
            !program.is_null(),
            "compile failed: {}",
            diag_message(&diag)
        );
        let program = ProgramHandle(program);

        let instance = onda_instance_create(program.0, 0, 1, &mut *diag);
        assert!(
            !instance.is_null(),
            "instance create failed: {}",
            diag_message(&diag)
        );
        let instance = InstanceHandle(instance);

        let mut out = vec![0.0_f32; 128];
        assert_eq!(
            onda_bind_output(
                instance.0,
                0,
                out.as_mut_ptr().cast::<c_void>(),
                (out.len() * std::mem::size_of::<f32>()) as i32,
            ),
            0
        );
        assert_eq!(onda_process_checked(instance.0, 128), 0);
        for sample in out {
            assert!((sample - 0.25).abs() < 1e-6);
        }
    }
}

#[test]
fn c_api_process_checked_accepts_sub_block_frame_counts() {
    unsafe {
        let frames = 512_i32;
        let sub_frames = 128_i32;
        let program = compile_program(
            r#"
outs { out1 }
sample { out1 = 0.25 }
"#,
        );

        let mut diag = empty_diag();
        let instance = onda_instance_create(program.0, 0, 1, &mut *diag);
        assert!(
            !instance.is_null(),
            "instance create failed: {}",
            diag_message(&diag)
        );
        let instance = InstanceHandle(instance);

        let mut out = vec![-1.0_f32; frames as usize];
        assert_eq!(
            onda_bind_output(
                instance.0,
                0,
                out.as_mut_ptr().cast::<c_void>(),
                (out.len() * std::mem::size_of::<f32>()) as i32,
            ),
            0
        );
        assert_eq!(onda_process_checked(instance.0, sub_frames), 0);
        for sample in &out[..sub_frames as usize] {
            assert!((*sample - 0.25).abs() < 1e-6);
        }
        for sample in &out[sub_frames as usize..] {
            assert_eq!(*sample, -1.0);
        }
    }
}

#[test]
fn c_api_process_checked_segment_gates_block_hooks() {
    unsafe {
        let frames = 512_i32;
        let segment_frames = 128_i32;
        let program = compile_program(
            r#"
outs { out1 }
init {
  pre = 0.0
  post = 0.0
}
block {
  pre = pre + 1.0
  sample {
    out1 = pre * 100.0 + post
  }
  post = post + 1.0
}
"#,
        );

        let mut diag = empty_diag();
        let instance = onda_instance_create(program.0, 0, 1, &mut *diag);
        assert!(
            !instance.is_null(),
            "instance create failed: {}",
            diag_message(&diag)
        );
        let instance = InstanceHandle(instance);

        let mut out = vec![0.0_f32; frames as usize];
        assert_eq!(
            onda_bind_output(
                instance.0,
                0,
                out.as_mut_ptr().cast::<c_void>(),
                (out.len() * std::mem::size_of::<f32>()) as i32,
            ),
            0
        );

        assert_eq!(
            onda_process_checked_segment(instance.0, 0, segment_frames, ONDA_PROCESS_BEGIN_BLOCK),
            0
        );
        for sample in &out[..segment_frames as usize] {
            assert!((*sample - 100.0).abs() < 1e-6);
        }

        out.fill(0.0);
        assert_eq!(
            onda_process_checked_segment(
                instance.0,
                segment_frames,
                segment_frames,
                ONDA_PROCESS_END_BLOCK
            ),
            0
        );
        for sample in &out[..segment_frames as usize] {
            assert!(sample.abs() < 1e-6);
        }
        for sample in &out[segment_frames as usize..(segment_frames * 2) as usize] {
            assert!((*sample - 100.0).abs() < 1e-6);
        }

        out.fill(0.0);
        assert_eq!(
            onda_process_checked_segment(instance.0, 0, frames, ONDA_PROCESS_FULL_BLOCK),
            0
        );
        for sample in &out {
            assert!((*sample - 201.0).abs() < 1e-6);
        }

        assert_eq!(
            onda_process_checked_segment(instance.0, 0, frames, 1 << 8),
            -2
        );
        assert_eq!(
            onda_process_checked_segment(instance.0, frames - 1, 2, ONDA_PROCESS_END_BLOCK),
            -2
        );
    }
}

#[test]
fn c_api_process_checked_segment_uses_start_frame_for_io() {
    unsafe {
        let frames = 512_i32;
        let start_frame = 128_i32;
        let segment_frames = 2_i32;
        let program = compile_program(
            r#"
ins { in1 }
outs { out1 }
sample {
  out1 = in1 + 10.0
}
"#,
        );

        let mut diag = empty_diag();
        let instance = onda_instance_create(program.0, 1, 1, &mut *diag);
        assert!(
            !instance.is_null(),
            "instance create failed: {}",
            diag_message(&diag)
        );
        let instance = InstanceHandle(instance);

        let input = (0..frames).map(|frame| frame as f32).collect::<Vec<_>>();
        let mut out = vec![0.0_f32; frames as usize];
        assert_eq!(
            onda_bind_input(
                instance.0,
                0,
                input.as_ptr().cast::<c_void>(),
                (input.len() * std::mem::size_of::<f32>()) as i32,
            ),
            0
        );
        assert_eq!(
            onda_bind_output(
                instance.0,
                0,
                out.as_mut_ptr().cast::<c_void>(),
                (out.len() * std::mem::size_of::<f32>()) as i32,
            ),
            0
        );

        assert_eq!(
            onda_process_checked_segment(
                instance.0,
                start_frame,
                segment_frames,
                ONDA_PROCESS_FULL_BLOCK
            ),
            0
        );
        assert!(out[..start_frame as usize]
            .iter()
            .all(|sample| sample.abs() < 1e-6));
        assert_eq!(out[start_frame as usize], 138.0);
        assert_eq!(out[start_frame as usize + 1], 139.0);
        assert!(out[start_frame as usize + segment_frames as usize..]
            .iter()
            .all(|sample| sample.abs() < 1e-6));
    }
}

#[test]
fn c_api_process_unchecked_segment_gates_block_hooks() {
    unsafe {
        let frames = 512_i32;
        let segment_frames = 128_i32;
        let program = compile_program(
            r#"
outs { out1 }
init {
  pre = 0.0
  post = 0.0
}
block {
  pre = pre + 1.0
  sample {
    out1 = pre * 100.0 + post
  }
  post = post + 1.0
}
"#,
        );

        let mut diag = empty_diag();
        let instance = onda_instance_create(program.0, 0, 1, &mut *diag);
        assert!(
            !instance.is_null(),
            "instance create failed: {}",
            diag_message(&diag)
        );
        let instance = InstanceHandle(instance);

        let mut out = vec![0.0_f32; frames as usize];
        assert_eq!(
            onda_bind_output(
                instance.0,
                0,
                out.as_mut_ptr().cast::<c_void>(),
                (out.len() * std::mem::size_of::<f32>()) as i32,
            ),
            0
        );
        assert_eq!(onda_prepare_unchecked_process(instance.0), 0);

        assert_eq!(
            onda_process_unchecked_segment(instance.0, 0, segment_frames, ONDA_PROCESS_BEGIN_BLOCK),
            0
        );
        for sample in &out[..segment_frames as usize] {
            assert!((*sample - 100.0).abs() < 1e-6);
        }

        out.fill(0.0);
        assert_eq!(
            onda_process_unchecked_segment(
                instance.0,
                segment_frames,
                segment_frames,
                ONDA_PROCESS_END_BLOCK
            ),
            0
        );
        for sample in &out[..segment_frames as usize] {
            assert!(sample.abs() < 1e-6);
        }
        for sample in &out[segment_frames as usize..(segment_frames * 2) as usize] {
            assert!((*sample - 100.0).abs() < 1e-6);
        }

        out.fill(0.0);
        assert_eq!(
            onda_process_unchecked_segment(instance.0, 0, frames, ONDA_PROCESS_FULL_BLOCK),
            0
        );
        for sample in &out {
            assert!((*sample - 201.0).abs() < 1e-6);
        }
    }
}

#[test]
fn c_api_compile_options_sample_rate_controls_builtin_sample_rate() {
    unsafe {
        let src = CString::new(
            r#"
outs { out1 }
sample { out1 = SAMPLE_RATE }
"#,
        )
        .expect("source contains no NUL bytes");

        let sample_rate = 12_345.0_f32;
        let block_size = 64_i32;
        let options = onda_compile_options_t {
            fast_math: 0,
            sample_rate,
            block_size,
        };
        let mut diag = empty_diag();
        let program = onda_compile(src.as_ptr(), &options, &mut *diag);
        assert!(
            !program.is_null(),
            "compile failed: {}",
            diag_message(&diag)
        );
        let program = ProgramHandle(program);

        let instance = onda_instance_create(program.0, 0, 1, &mut *diag);
        assert!(
            !instance.is_null(),
            "instance create failed: {}",
            diag_message(&diag)
        );
        let instance = InstanceHandle(instance);

        let mut out = vec![0.0_f32; block_size as usize];
        assert_eq!(
            onda_bind_output(
                instance.0,
                0,
                out.as_mut_ptr().cast::<c_void>(),
                (out.len() * std::mem::size_of::<f32>()) as i32,
            ),
            0
        );
        assert_eq!(onda_process_checked(instance.0, block_size), 0);
        for sample in out {
            assert!((sample - sample_rate).abs() < 1e-3);
        }
    }
}

#[test]
fn c_api_unchecked_process_returns_generated_runtime_failure_code() {
    unsafe {
        let program = compile_program(
            r#"
params:
  divisor: i32 = 0

def quotient(value: i32, by: i32):
  return value / by

sample:
  out1 = f32(quotient(i32(1), divisor))
"#,
        );
        let mut diag = empty_diag();
        let instance = onda_instance_create(program.0, 0, 1, &mut *diag);
        assert!(
            !instance.is_null(),
            "instance create failed: {}",
            diag_message(&diag)
        );
        let instance = InstanceHandle(instance);
        let mut output = vec![0.0_f32; 512];
        assert_eq!(
            onda_bind_output(
                instance.0,
                0,
                output.as_mut_ptr().cast::<c_void>(),
                (output.len() * std::mem::size_of::<f32>()) as i32,
            ),
            0
        );
        assert_eq!(onda_prepare_unchecked_process(instance.0), 0);
        assert_eq!(
            onda_process_unchecked_segment(instance.0, 0, 1, ONDA_PROCESS_BEGIN_BLOCK),
            ONDA_EXECUTION_RUNTIME_SAFETY_FAILURE,
            "generated runtime failures must cross the C ABI with their named status code"
        );
    }
}

#[test]
fn c_api_buffer_may_write_metadata_tracks_reachable_writes() {
    unsafe {
        let program = compile_program(
            r#"
outs { out1 }
buffers {
  write_buf: buffer<f32>
  read_buf: buffer<f32>
}
def touch(buf: buffer<f32>):
  buf[0] = 0.75
proc Writer:
  ins:
    in1
  buffers:
    b: buffer<f32>
  outs:
    out1
  sample:
    touch(b)
    out1 = in1
init:
  w = Writer(b = write_buf)
sample:
  out1 = w(read_buf[0])
"#,
        );

        let write_name = CString::new("write_buf").expect("valid cstr");
        let read_name = CString::new("read_buf").expect("valid cstr");
        let write_idx = onda_buffer_index(program.0, write_name.as_ptr());
        let read_idx = onda_buffer_index(program.0, read_name.as_ptr());
        assert!(write_idx >= 0);
        assert!(read_idx >= 0);

        assert_eq!(onda_buffer_may_write(program.0, write_idx), 1);
        assert_eq!(onda_buffer_may_write(program.0, read_idx), 0);
        assert_eq!(onda_buffer_may_write(program.0, -1), -1);
        assert_eq!(onda_buffer_may_write(std::ptr::null(), write_idx), -1);
    }
}

#[test]
fn c_api_buffer_may_write_marks_conditional_and_multichannel_writes() {
    unsafe {
        let program = compile_program(
            r#"
outs { out1 }
buffers {
  branch_buf: buffer<f32>
  stereo_buf: buffer<f32[2]>
  read_buf: buffer<f32>
}
def write_if(buf: buffer<f32>):
  if (1 > 0):
    buf[0] = 1.0
proc StereoWriter:
  ins:
    in1
  outs:
    out1
  buffers:
    b: buffer<f32[2]>
  sample:
    b[0, 0] = 0.1
    out1 = in1
init:
  sw = StereoWriter(b = stereo_buf)
sample:
  write_if(branch_buf)
  out1 = sw(read_buf[0])
"#,
        );

        let branch_name = CString::new("branch_buf").expect("valid cstr");
        let stereo_name = CString::new("stereo_buf").expect("valid cstr");
        let read_name = CString::new("read_buf").expect("valid cstr");
        let branch_idx = onda_buffer_index(program.0, branch_name.as_ptr());
        let stereo_idx = onda_buffer_index(program.0, stereo_name.as_ptr());
        let read_idx = onda_buffer_index(program.0, read_name.as_ptr());
        assert!(branch_idx >= 0);
        assert!(stereo_idx >= 0);
        assert!(read_idx >= 0);

        assert_eq!(onda_buffer_may_write(program.0, branch_idx), 1);
        assert_eq!(onda_buffer_may_write(program.0, stereo_idx), 1);
        assert_eq!(onda_buffer_may_write(program.0, read_idx), 0);
    }
}

#[test]
fn c_api_buffer_may_write_is_true_when_buffer_is_read_and_written() {
    unsafe {
        let program = compile_program(
            r#"
outs { out1 }
buffers {
  rw_buf: buffer<f32>
}
sample:
  x = rw_buf[0]
  rw_buf[0] = x
  out1 = x
"#,
        );

        let rw_name = CString::new("rw_buf").expect("valid cstr");
        let rw_idx = onda_buffer_index(program.0, rw_name.as_ptr());
        assert!(rw_idx >= 0);
        assert_eq!(onda_buffer_may_write(program.0, rw_idx), 1);
    }
}

#[test]
fn c_api_buffer_may_write_tracks_method_style_buffer_calls() {
    unsafe {
        let program = compile_program(
            r#"
outs { out1 }
buffers {
  method_write_buf: buffer<f32>
  method_read_buf: buffer<f32>
}
def touch(buf: buffer<f32>):
  buf[0] = 0.5
proc Writer:
  ins:
    in1
  outs:
    out1
  buffers:
    b: buffer<f32>
  sample:
    touch(b)
    out1 = in1
init:
  w = Writer(b = method_write_buf)
sample:
  out1 = w(method_read_buf[0])
"#,
        );

        let write_name = CString::new("method_write_buf").expect("valid cstr");
        let read_name = CString::new("method_read_buf").expect("valid cstr");
        let write_idx = onda_buffer_index(program.0, write_name.as_ptr());
        let read_idx = onda_buffer_index(program.0, read_name.as_ptr());
        assert!(write_idx >= 0);
        assert!(read_idx >= 0);
        assert_eq!(onda_buffer_may_write(program.0, write_idx), 1);
        assert_eq!(onda_buffer_may_write(program.0, read_idx), 0);
    }
}

#[test]
fn c_api_unbound_buffers_remain_processable_with_neutral_descriptors() {
    unsafe {
        let program = compile_program(
            r#"
outs { out1 }
buffers { samples: buffer<f32> }
sample { out1 = 0.25 }
"#,
        );
        let mut diag = empty_diag();
        let instance = onda_instance_create(program.0, 0, 1, &mut *diag);
        assert!(
            !instance.is_null(),
            "instance create failed: {}",
            diag_message(&diag)
        );
        let instance = InstanceHandle(instance);

        let mut output = vec![0.0_f32; 512];
        assert_eq!(
            onda_bind_output(
                instance.0,
                0,
                output.as_mut_ptr().cast::<c_void>(),
                std::mem::size_of_val(output.as_slice()) as i32,
            ),
            0
        );
        assert_eq!(
            onda_bind_buffer(instance.0, 0, std::ptr::null_mut(), 0, 0, 48_000.0, 0,),
            0
        );
        assert_eq!(onda_process_checked(instance.0, 512), 0);
        assert_eq!(onda_reset_buffer_to_project_default(instance.0, 0), -2);

        let mut samples = [1.0_f32];
        assert_eq!(
            onda_bind_buffer(
                instance.0,
                0,
                samples.as_mut_ptr().cast::<c_void>(),
                1,
                1,
                48_000.0,
                0,
            ),
            0
        );
        assert_eq!(onda_process_checked(instance.0, 512), 0);
        assert!(output.iter().all(|sample| (*sample - 0.25).abs() < 1e-6));

        assert_eq!(
            onda_bind_buffer(
                instance.0,
                0,
                samples.as_mut_ptr().cast::<c_void>(),
                -1,
                -1,
                0.0,
                0,
            ),
            0
        );
        assert_eq!(onda_prepare_unchecked_process(instance.0), 0);

        assert_eq!(
            onda_bind_buffer(instance.0, 0, std::ptr::null_mut(), 1, 1, 48_000.0, 0,),
            -2
        );
    }
}

#[test]
fn c_api_exposes_contiguous_buffer_array_groups() {
    unsafe {
        let program = compile_program(
            r#"
buffers:
  bank: f32 {3}
  tail: buffer<f32>
sample:
  out1 = bank[1][0]
"#,
        );
        assert_eq!(onda_buffer_count(program.0), 4);
        assert_eq!(onda_buffer_array_count(program.0), 1);
        assert_eq!(onda_buffer_array_first(program.0, 0), 0);
        assert_eq!(onda_buffer_array_len(program.0, 0), 3);
        assert_eq!(
            CStr::from_ptr(onda_buffer_array_name(program.0, 0)).to_str(),
            Ok("bank")
        );
        assert_eq!(
            CStr::from_ptr(onda_buffer_name(program.0, 2)).to_str(),
            Ok("bank[2]")
        );
        assert_eq!(onda_buffer_array_len(program.0, 1), -1);
    }
}

#[test]
fn c_api_buffer_may_write_indexes_physical_buffer_array_slots() {
    unsafe {
        let program = compile_program(
            r#"
buffers:
  bank: f32 {4}
sample:
  bank[2][0] = 1.0
"#,
        );
        let first = onda_buffer_array_first(program.0, 0);
        assert_eq!(first, 0);
        assert_eq!(onda_buffer_array_len(program.0, 0), 4);
        assert_eq!(
            (0..4)
                .map(|slot| onda_buffer_may_write(program.0, first + slot))
                .collect::<Vec<_>>(),
            vec![0, 0, 1, 0]
        );
    }
}

#[test]
fn c_api_infers_local_from_primitive_array_index_read_in_sample() {
    unsafe {
        let _program = compile_program(
            r#"
ins { in1 }
outs { out1 }
params {
  delayTime = 0.2 {0.01, 0.5}
}
init {
  line: f32[SR]
  writePos: i32 = 0
  lineLen: i32 = i32(SR)
}
sample {
  delaySamples: i32 = i32(delayTime * SR)
  if (delaySamples < 1) { delaySamples = 1 }
  if (delaySamples >= lineLen) { delaySamples = lineLen - 1 }

  readPos: i32 = writePos - delaySamples
  if (readPos < 0) { readPos = readPos + lineLen }

  delayed = line[readPos]
  line[writePos] = in1
  writePos = writePos + 1
  if (writePos >= lineLen) { writePos = 0 }
  out1 = delayed
}
"#,
        );
    }
}

#[test]
fn c_api_pi_and_two_pi_use_f64_precision_constants() {
    unsafe {
        let frames = 512_i32;
        let program = compile_program(
            r#"
outs {
  out1
  out2
}
sample {
  out1 = f32(PI - f64(f32(PI)))
  out2 = f32(TWO_PI - f64(f32(TWO_PI)))
}
"#,
        );

        let mut diag = empty_diag();
        let instance = onda_instance_create(program.0, 0, 2, &mut *diag);
        assert!(
            !instance.is_null(),
            "instance create failed: {}",
            diag_message(&diag)
        );
        let instance = InstanceHandle(instance);

        let mut out1 = vec![0.0_f32; frames as usize];
        let mut out2 = vec![0.0_f32; frames as usize];
        assert_eq!(
            onda_bind_output(
                instance.0,
                0,
                out1.as_mut_ptr().cast::<c_void>(),
                (out1.len() * std::mem::size_of::<f32>()) as i32,
            ),
            0
        );
        assert_eq!(
            onda_bind_output(
                instance.0,
                1,
                out2.as_mut_ptr().cast::<c_void>(),
                (out2.len() * std::mem::size_of::<f32>()) as i32,
            ),
            0
        );
        assert_eq!(onda_process_checked(instance.0, frames), 0);

        for sample in out1 {
            assert!(sample.abs() > 1e-9);
        }
        for sample in out2 {
            assert!(sample.abs() > 1e-9);
        }
    }
}

#[test]
fn c_api_block_size_builtin_is_i32_typed() {
    unsafe {
        let frames = 512_i32;
        let program = compile_program(
            r#"
outs { out1 }
init {
  bs: i32 = BLOCK_SIZE
}
sample {
  out1 = f32(bs)
}
"#,
        );

        let mut diag = empty_diag();
        let instance = onda_instance_create(program.0, 0, 1, &mut *diag);
        assert!(
            !instance.is_null(),
            "instance create failed: {}",
            diag_message(&diag)
        );
        let instance = InstanceHandle(instance);

        let mut out = vec![0.0_f32; frames as usize];
        assert_eq!(
            onda_bind_output(
                instance.0,
                0,
                out.as_mut_ptr().cast::<c_void>(),
                (out.len() * std::mem::size_of::<f32>()) as i32,
            ),
            0
        );
        assert_eq!(onda_process_checked(instance.0, frames), 0);
        for sample in out {
            assert!((sample - 512.0).abs() < 1e-6);
        }
    }
}
