#ifndef ONDA_H
#define ONDA_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct onda_program onda_program_t;
typedef struct onda_instance onda_instance_t;
typedef struct onda_source_manifest onda_source_manifest_t;
typedef struct onda_project_image onda_project_image_t;
typedef struct onda_project_materialization_plan onda_project_materialization_plan_t;

/* Primitive element type identifiers used by metadata and buffer binding APIs. */
enum {
  ONDA_PRIMITIVE_F32 = 0,
  ONDA_PRIMITIVE_F64 = 1,
  ONDA_PRIMITIVE_I32 = 2,
  ONDA_PRIMITIVE_I64 = 3,
  ONDA_PRIMITIVE_BOOL = 4
};

enum {
  ONDA_PARAM_SCALE_LINEAR = 0,
  ONDA_PARAM_SCALE_LOG = 1
};

/* Largest integer magnitude represented exactly by the f64 host-control API. */
#define ONDA_MAX_EXACT_HOST_CONTROL_INTEGER INT64_C(9007199254740991)

/* Buffer channel declaration kinds used by buffer metadata queries. */
enum {
  ONDA_BUFFER_CHANNELS_MONO = 0,
  ONDA_BUFFER_CHANNELS_STATIC = 1,
  ONDA_BUFFER_CHANNELS_DYNAMIC = 2
};

/* Diagnostic payload returned from API failures.
   Every non-NULL string is owned by Onda. Zero-initialize this structure before
   first use, call onda_diag_dispose before reusing it as an output, and call
   onda_diag_dispose once more when the diagnostic is no longer needed. */
typedef struct {
  int code;
  int line;
  int column;
  int end_line;
  int end_column;
  const char* message;
  const char* file;
  const char* trace;
} onda_diag_t;

/* Releases all strings in diag and resets it to zero. Safe to call with NULL
   or repeatedly on the same diagnostic. Copies of a diagnostic share the same
   strings and must not be disposed independently. */
void onda_diag_dispose(onda_diag_t* diag);

/* Compile options for onda_compile. */
typedef struct {
  /* fast_math != 0 enables LLVM fast-math lowering. */
  int fast_math;
  /* Compile-time sample rate constant. Must be finite and > 0. */
  float sample_rate;
  /* Fixed compile-time block size. Must be > 0. */
  int block_size;
} onda_compile_options_t;

typedef struct {
  /* NUL-terminated opaque source identity. */
  const char* path_utf8;
  /* Exact UTF-8 source bytes; source_bytes determines the length.
     May be NULL when source_bytes is zero. */
  const char* source_utf8;
  size_t source_bytes;
} onda_source_graph_document_t;

enum {
  ONDA_SOURCE_REFERENCE_INCLUDE = 0,
  ONDA_SOURCE_REFERENCE_IMPORT = 1
};

typedef struct {
  const char* source_path_utf8;
  int kind;
  const char* specifier_utf8;
  const char* target_path_utf8;
} onda_source_graph_resolution_t;

typedef struct {
  int kind;
  const char* specifier_utf8;
  const char* replacement_utf8;
} onda_source_rewrite_t;

/* One logical buffer binding supplied while capturing a project image.
   ondabuffer_bytes must contain one canonical .ondabuffer asset. */
typedef struct {
  const char* name_utf8;
  const void* ondabuffer_bytes;
  size_t ondabuffer_byte_count;
} onda_project_buffer_asset_t;

typedef struct {
  const char* path_utf8;
  const void* bytes;
  size_t byte_count;
} onda_project_file_t;

typedef struct {
  int element_type;
  uint32_t frames;
  uint32_t channels;
  float sample_rate;
  size_t sample_bytes;
} onda_buffer_asset_info_t;

typedef void* (*onda_alloc_fn)(void* context, size_t size, size_t align);
typedef void (*onda_free_fn)(void* context, void* ptr, size_t size, size_t align);

/* Host allocator used by custom instance creation.
   Onda calls alloc only synchronously during onda_instance_create_with_allocator; no operation on
   a successfully created instance calls alloc. free may be called during failed creation or later
   instance destruction. The context and callbacks must remain valid until every instance created
   with this allocator has been destroyed. alloc must be callable on each thread where the host
   creates an instance. free must be callable on every thread where creation can fail or an instance
   can be destroyed. When multiple instances share an allocator and are created or destroyed
   concurrently, the corresponding callbacks must support those concurrent calls. */
typedef struct {
  void* context;
  onda_alloc_fn alloc;
  onda_free_fn free;
} onda_allocator_t;

/* Compiles Onda source and returns a program handle, or NULL on failure. */
onda_program_t* onda_compile(
  const char* src_utf8,
  const onda_compile_options_t* options,
  onda_diag_t* out_diag
);
/* Compiles an editable filesystem .onda, .on, or .ondaproject input and returns
   a program handle, or NULL on failure. A project input resolves its entry and
   retains inline and file-backed project buffers as immutable program defaults.
   Relative include/import resolution uses the resolved source entry path.
   Filesystem-backed inputs, source dependencies, and project assets must not
   traverse symbolic links.
   When out_sources is non-NULL, it receives an owned manifest on success or
   failure containing every non-stdlib source resolved before compilation
   stopped, plus unresolved candidates and the complete filesystem watch set.
   Destroy it with onda_source_manifest_destroy. */
onda_program_t* onda_compile_file(
  const char* file_path_utf8,
  const onda_compile_options_t* options,
  onda_source_manifest_t** out_sources,
  onda_diag_t* out_diag
);
/* Compiles the reachable portion of an exact in-memory source graph without
   consulting the filesystem. Source paths are NUL-terminated opaque UTF-8
   identities. Each non-stdlib import/include encountered while parsing must
   have one matching resolution. Unreferenced sources and resolutions are
   permitted and are not included in the returned manifest. */
onda_program_t* onda_compile_source_graph(
  const char* entry_path_utf8,
  const onda_source_graph_document_t* sources,
  size_t source_count,
  const onda_source_graph_resolution_t* resolutions,
  size_t resolution_count,
  const onda_compile_options_t* options,
  onda_source_manifest_t** out_sources,
  onda_diag_t* out_diag
);
/* Rewrites parsed top-level non-stdlib include/import specifiers in one exact
   UTF-8 source document. All such references in the source must have a
   matching rewrite; built-in std imports are preserved. Returns the required
   byte count, or -1 on failure. If out_utf8 is NULL or out_capacity is too
   small, no bytes are copied. Output is exact UTF-8 and is not NUL-terminated. */
int onda_rewrite_source_references(
  const char* source_path_utf8,
  const char* source_utf8,
  size_t source_bytes,
  const onda_source_rewrite_t* rewrites,
  size_t rewrite_count,
  char* out_utf8,
  int out_capacity,
  onda_diag_t* out_diag
);
/* Returns the number of contributing source files, or -1 for an invalid handle. */
int onda_source_manifest_count(const onda_source_manifest_t* manifest);
/* Returns one NUL-terminated UTF-8 source identity, or NULL for an invalid
   index/handle. onda_compile_file returns absolute canonical filesystem paths;
   onda_compile_source_graph preserves the supplied opaque identities. The pointer
   remains valid until onda_source_manifest_destroy. */
const char* onda_source_manifest_path(
  const onda_source_manifest_t* manifest,
  int index
);
/* Returns the number of referenced non-stdlib paths which did not resolve,
   or -1 for an invalid handle. */
int onda_source_manifest_unresolved_count(const onda_source_manifest_t* manifest);
/* Returns one absolute normalized UTF-8 unresolved candidate path, or NULL for
   an invalid index/handle. The pointer remains valid until manifest destruction. */
const char* onda_source_manifest_unresolved_path(
  const onda_source_manifest_t* manifest,
  int index
);
/* Returns the exact filesystem paths whose contents or existence may affect a
   repeated onda_compile_file call. The set includes the selected input,
   resolved and unresolved sources, and, for .ondaproject inputs, its manifest,
   declared entry, and file-backed buffer assets. Missing dependency, entry,
   and asset paths are retained for recovery. Paths are unique and remain valid
   until manifest destruction. The count returns -1 for an invalid handle; the
   path getter returns NULL for an invalid handle/index. In-memory source-graph
   manifests have an empty watch set. */
int onda_source_manifest_watch_count(const onda_source_manifest_t* manifest);
const char* onda_source_manifest_watch_path(
  const onda_source_manifest_t* manifest,
  int index
);
/* Returns the number of resolved source documents with captured contents.
   Document paths and contents remain valid until manifest destruction. */
int onda_source_manifest_document_count(const onda_source_manifest_t* manifest);
const char* onda_source_manifest_document_path(
  const onda_source_manifest_t* manifest,
  int index
);
/* Returns a pointer to exact source bytes and writes their length to out_bytes,
   or NULL for an invalid index/handle. out_bytes may be NULL. The contents need
   not be NUL-terminated and remain valid until manifest destruction. */
const char* onda_source_manifest_document_contents(
  const onda_source_manifest_t* manifest,
  int index,
  size_t* out_bytes
);
/* Returns the successful non-stdlib import/include resolution count.
   String results remain valid until manifest destruction. */
int onda_source_manifest_resolution_count(const onda_source_manifest_t* manifest);
const char* onda_source_manifest_resolution_source_path(
  const onda_source_manifest_t* manifest,
  int index
);
int onda_source_manifest_resolution_kind(
  const onda_source_manifest_t* manifest,
  int index
);
const char* onda_source_manifest_resolution_specifier(
  const onda_source_manifest_t* manifest,
  int index
);
const char* onda_source_manifest_resolution_target_path(
  const onda_source_manifest_t* manifest,
  int index
);
/* Returns unresolved non-stdlib references with their source identity,
   directive kind, original specifier, and candidate target identities.
   String results remain valid until manifest destruction. */
int onda_source_manifest_unresolved_resolution_count(
  const onda_source_manifest_t* manifest
);
const char* onda_source_manifest_unresolved_resolution_source_path(
  const onda_source_manifest_t* manifest,
  int index
);
int onda_source_manifest_unresolved_resolution_kind(
  const onda_source_manifest_t* manifest,
  int index
);
const char* onda_source_manifest_unresolved_resolution_specifier(
  const onda_source_manifest_t* manifest,
  int index
);
int onda_source_manifest_unresolved_resolution_candidate_count(
  const onda_source_manifest_t* manifest,
  int index
);
const char* onda_source_manifest_unresolved_resolution_candidate_path(
  const onda_source_manifest_t* manifest,
  int index,
  int candidate_index
);
/* Destroys a source manifest returned by onda_compile_file or
   onda_compile_source_graph. NULL is accepted. */
void onda_source_manifest_destroy(onda_source_manifest_t* manifest);

/* Canonical binary project format capabilities. */
int onda_project_image_format_version(void);
int onda_buffer_asset_format_version(void);
/* Stable until process exit. */
const char* onda_current_stdlib_digest(void);

/* Encodes host-native primitive samples as one canonical .ondabuffer asset.
   Returns the required byte count, or -1 on failure. Passing NULL output or
   insufficient capacity performs a size query without copying. */
int64_t onda_buffer_asset_encode(
  int element_type,
  uint32_t frames,
  uint32_t channels,
  float sample_rate,
  const void* samples,
  size_t sample_bytes,
  void* out_bytes,
  size_t out_capacity,
  onda_diag_t* out_diag
);
/* Validates and decodes one canonical .ondabuffer asset into host-native samples.
   out_info may be NULL. Return and output sizing follow onda_buffer_asset_encode. */
int64_t onda_buffer_asset_decode(
  const void* bytes,
  size_t byte_count,
  onda_buffer_asset_info_t* out_info,
  void* out_samples,
  size_t out_capacity,
  onda_diag_t* out_diag
);

/* Captures an exact successful source manifest, relocates it below source_root,
   and associates canonical typed buffer assets. The returned image owns all
   source and asset bytes and does not consult the filesystem when replayed. */
onda_project_image_t* onda_project_image_capture(
  const char* entry_path_utf8,
  const char* source_root_utf8,
  const onda_source_manifest_t* manifest,
  const onda_project_buffer_asset_t* buffers,
  size_t buffer_count,
  onda_diag_t* out_diag
);
onda_project_image_t* onda_project_image_deserialize(
  const void* bytes,
  size_t byte_count,
  onda_diag_t* out_diag
);
/* Loads an editable project from a complete set of relative files, validating
   and decoding every referenced .ondabuffer, WAV, and inline buffer into
   image-owned typed storage. Pass the selected .ondaproject path when the set
   contains multiple projects, or NULL to require an unambiguous manifest. */
onda_project_image_t* onda_project_image_load_files(
  const onda_project_file_t* files,
  size_t file_count,
  const char* project_file_path_utf8,
  onda_diag_t* out_diag
);
int64_t onda_project_image_serialize(
  const onda_project_image_t* image,
  void* out_bytes,
  size_t out_capacity,
  onda_diag_t* out_diag
);
/* Pointer remains valid until image destruction. */
const char* onda_project_image_content_digest(const onda_project_image_t* image);
/* Read-only inspection of the exact portable source graph. Returned strings
   remain valid until image destruction. Document contents are exact UTF-8,
   need not be NUL-terminated, and write their length to out_bytes when non-NULL. */
const char* onda_project_image_entry(const onda_project_image_t* image);
const char* onda_project_image_stdlib_digest(const onda_project_image_t* image);
int onda_project_image_document_count(const onda_project_image_t* image);
const char* onda_project_image_document_path(
  const onda_project_image_t* image,
  int index
);
const char* onda_project_image_document_contents(
  const onda_project_image_t* image,
  int index,
  size_t* out_bytes
);
int onda_project_image_resolution_count(const onda_project_image_t* image);
const char* onda_project_image_resolution_source(
  const onda_project_image_t* image,
  int index
);
int onda_project_image_resolution_kind(
  const onda_project_image_t* image,
  int index
);
const char* onda_project_image_resolution_specifier(
  const onda_project_image_t* image,
  int index
);
const char* onda_project_image_resolution_target(
  const onda_project_image_t* image,
  int index
);
/* Read-only logical buffer bindings and canonical asset metadata. Buffer names
   and asset IDs remain valid until image destruction. Invalid indices return
   NULL, -1, or NaN as appropriate. */
int onda_project_image_buffer_count(const onda_project_image_t* image);
const char* onda_project_image_buffer_name(
  const onda_project_image_t* image,
  int index
);
const char* onda_project_image_buffer_asset_id(
  const onda_project_image_t* image,
  int index
);
int onda_project_image_buffer_element_type(
  const onda_project_image_t* image,
  int index
);
int64_t onda_project_image_buffer_frames(
  const onda_project_image_t* image,
  int index
);
int64_t onda_project_image_buffer_channels(
  const onda_project_image_t* image,
  int index
);
float onda_project_image_buffer_sample_rate(
  const onda_project_image_t* image,
  int index
);
/* Compiles the image and retains its decoded buffer assets as immutable program
   defaults. Compilation fails if reachable Onda code may write a project-bound
   buffer. Instances automatically use these defaults until the host replaces
   or unbinds them. */
onda_program_t* onda_project_image_compile(
  const onda_project_image_t* image,
  const onda_compile_options_t* options,
  onda_diag_t* out_diag
);
onda_project_materialization_plan_t* onda_project_image_materialize(
  const onda_project_image_t* image,
  onda_diag_t* out_diag
);
int onda_project_materialization_file_count(
  const onda_project_materialization_plan_t* plan
);
/* Pointer remains valid until plan destruction. */
const char* onda_project_materialization_file_path(
  const onda_project_materialization_plan_t* plan,
  int index
);
int64_t onda_project_materialization_file_bytes(
  const onda_project_materialization_plan_t* plan,
  int index,
  void* out_bytes,
  size_t out_capacity
);
void onda_project_materialization_destroy(onda_project_materialization_plan_t* plan);
void onda_project_image_destroy(onda_project_image_t* image);

/* Destroys a program handle created by onda_compile, onda_compile_file,
   onda_compile_source_graph, or onda_project_image_compile.
   Programs are immutable and may be queried or used to create instances concurrently.
   Destruction is not realtime-safe. */
void onda_program_destroy(onda_program_t* program);

/* Creates a runtime instance for a compiled program, or NULL on failure.
   Uses compile-time sample_rate and block_size captured in the program handle.
   Programs compiled from filesystem .ondaproject inputs or project images
   automatically bind their immutable project buffer defaults. The instance
   retains the compiled program and its defaults, so the original program and
   project-image handles may be destroyed afterward. */
onda_instance_t* onda_instance_create(
  const onda_program_t* program,
  int in_channels,
  int out_channels,
  onda_diag_t* out_diag
);
/* Creates a runtime instance whose instance-owned storage uses allocator.
   The returned instance still retains a reference to the compiled program internally;
   compiled/JIT memory is released when the last program/instance reference is gone.
   Immutable project defaults are bound exactly as for onda_instance_create.
   allocator->alloc and allocator->free must be non-NULL and must honor size/align. */
onda_instance_t* onda_instance_create_with_allocator(
  const onda_program_t* program,
  int in_channels,
  int out_channels,
  const onda_allocator_t* allocator,
  onda_diag_t* out_diag
);
/* Destroys an instance handle created by onda_instance_create or onda_instance_create_with_allocator.
   An instance has one exclusive owner at a time. It may be transferred between threads, but no
   instance operation may overlap another operation on the same handle. Destruction is not
   realtime-safe. */
void onda_instance_destroy(onda_instance_t* instance);

/* Sets a parameter by index from raw bytes; returns 0 on success, negative on error. */
int onda_set_param_by_index(
  onda_instance_t* instance,
  int index,
  const void* value_ptr,
  int value_bytes
);
/* Sets a scalar parameter in its plain domain, clamping and snapping as declared. */
int onda_set_param_plain_f64(onda_instance_t* instance, int index, double plain);
/* Sets a scalar parameter from a normalized host value in [0, 1]. */
int onda_set_param_normalized(onda_instance_t* instance, int index, double normalized);

/* Triggers one event by index with packed payload bytes; returns 0 on success, negative on error.
   Unknown event indices are ignored and return success. */
int onda_trigger_event_by_index(
  onda_instance_t* instance,
  int index,
  const void* payload_ptr,
  int payload_bytes
);
/* Triggers one event without payload/binding validation; unsafe if payload/buffer metadata is
   invalid. Returns 0 on success, a positive generated-runtime failure code, or a negative API
   error. */
int onda_trigger_event_by_index_unchecked(
  onda_instance_t* instance,
  int index,
  const void* payload_ptr,
  int payload_bytes
);

/* Binds one input entry to host memory; returns 0 on success, negative on error.
   Zero-copy contract: runtime stores src_ptr and reads from it directly (no internal copy).
   src_ptr must remain valid, correctly sized, and at a stable address until this slot is
   rebound/unbound (null + 0 bytes) or the instance is destroyed.
   src_ptr memory must be readable during processing and naturally aligned for the input's
   declared primitive element type; misaligned bindings are rejected.
   Contract for optimized codegen: bound input/output/buffer memory regions must not overlap. */
int onda_bind_input(
  onda_instance_t* instance,
  int index,
  const void* src_ptr,
  int src_bytes
);

/* Binds one output entry to host memory; returns 0 on success, negative on error.
   Zero-copy contract: runtime stores dst_ptr and writes to it directly (no internal copy).
   dst_ptr must remain valid, correctly sized, and at a stable address until this slot is
   rebound/unbound (null + 0 bytes) or the instance is destroyed.
   dst_ptr memory must be writable during processing and naturally aligned for the output's
   declared primitive element type; misaligned bindings are rejected.
   Contract for optimized codegen: bound input/output/buffer memory regions must not overlap. */
int onda_bind_output(
  onda_instance_t* instance,
  int index,
  void* dst_ptr,
  int dst_bytes
);

/* Binds one buffer entry; elem_type must be an ONDA_PRIMITIVE_* value.
   Zero-copy contract: runtime stores ptr and accesses it directly (no internal copy).
   sample_rate == 0 unbinds the slot regardless of ptr and shape. Null + 0 frames + 0 channels also
   unbinds the slot, regardless of sample_rate. An unbound slot remains processable through neutral
   one-frame storage: reads return zero and writes are discarded. Otherwise, ptr must be non-null and
   remain valid, correctly sized for positive frame/channel counts, and at a stable address until
   this slot is rebound/unbound or the instance is destroyed.
   ptr memory must be readable during processing and naturally aligned for elem_type;
   it must also be writable when the buffer declaration permits writes. Misaligned
   bindings are rejected.
   Contract for optimized codegen: bound input/output/buffer memory regions must not overlap. */
int onda_bind_buffer(
  onda_instance_t* instance,
  int index,
  void* ptr,
  int frames,
  int channels,
  float sample_rate,
  int elem_type
);

/* Restores the immutable project asset associated with a buffer slot. Returns
   0 on success, -1 for an invalid instance/index, or -2 when the program has
   no project defaults or the slot has no project default. */
int onda_reset_buffer_to_project_default(
  onda_instance_t* instance,
  int index
);

enum {
  ONDA_PROCESS_BEGIN_BLOCK = 1 << 0,
  ONDA_PROCESS_END_BLOCK = 1 << 1,
  ONDA_PROCESS_FULL_BLOCK = ONDA_PROCESS_BEGIN_BLOCK | ONDA_PROCESS_END_BLOCK
};

enum {
  ONDA_EXECUTION_OK = 0,
  ONDA_EXECUTION_RUNTIME_SAFETY_FAILURE = 1
};

/* Processes up to one logical block with current bindings and parameters; returns 0 on success.
   frames must be in [0, compile_time_block_size]. The runtime only reads/writes the first
   `frames` samples of each bound input/output entry for the current call. This convenience
   function runs block-pre and block-post hooks for this call. */
int onda_process_checked(onda_instance_t* instance, int frames);
/* Processes one segment of a logical block using full-block input/output bindings.
   The JIT loops local frames [0, frames) and reads/writes bound I/O at
   absolute frame start_frame + local_frame. Use ONDA_PROCESS_BEGIN_BLOCK on
   the first segment and ONDA_PROCESS_END_BLOCK on the final segment. A single
   unsplit block should pass start_frame=0 and ONDA_PROCESS_FULL_BLOCK. */
int onda_process_checked_segment(
  onda_instance_t* instance,
  int start_frame,
  int frames,
  int flags
);
/* Resets instance DSP/state memory to the initial post-init state captured at instance creation. */
int onda_reset_instance_state(onda_instance_t* instance);
/* Returns the byte size of the instance state snapshot, or -1 on invalid instance handle. */
int onda_instance_state_bytes(const onda_instance_t* instance);
/*
 * Copies the packed persistent-state snapshot. Compiler scratch and control-output mirrors are omitted.
 * If out_bytes is NULL or out_capacity is too small, no bytes are copied and the required size is returned.
 */
int onda_instance_snapshot_state(
  const onda_instance_t* instance,
  void* out_bytes,
  int out_capacity
);
/* Restores a packed snapshot and resets omitted scratch to its post-init state; returns 0 on success. */
int onda_instance_restore_state(
  onda_instance_t* instance,
  const void* bytes,
  int byte_count
);
/*
 * Copies the latest held value for one top-level kouts/control-output entry.
 * If out_bytes is NULL or out_capacity is too small, no bytes are copied and the required size is returned.
 */
int onda_control_output_read_bytes(
  const onda_instance_t* instance,
  int index,
  void* out_bytes,
  int out_capacity
);
/* Prepares current buffer descriptors, including neutral unbound slots, and validates required
   input/output bindings; returns 0 on success. */
int onda_validate_bindings(onda_instance_t* instance);
/* Validates input bindings only; returns 0 on success. */
int onda_validate_inputs(onda_instance_t* instance);
/* Validates output bindings only; returns 0 on success. */
int onda_validate_outputs(onda_instance_t* instance);
/* Prepares buffer descriptors, including neutral unbound slots; returns 0 on success. */
int onda_validate_buffers(onda_instance_t* instance);
/* Validates all bindings before unchecked processing. */
int onda_prepare_unchecked_process(onda_instance_t* instance);
/* Processes a full logical block without revalidation (unsafe if bindings are stale);
   returns 0 on success, a positive generated-runtime failure code, or a negative API error. */
int onda_process_unchecked(onda_instance_t* instance);
/* Processes one logical-block segment without revalidation. Use the same full-block
   binding, start_frame, frames, and flags contract as onda_process_checked_segment.
   Returns 0 on success, a positive generated-runtime failure code, or a negative API error. */
int onda_process_unchecked_segment(
  onda_instance_t* instance,
  int start_frame,
  int frames,
  int flags
);

/* Returns declared input entry count, or -1 on invalid program handle. */
int onda_input_count(const onda_program_t* program);
/* Returns declared output entry count, or -1 on invalid program handle. */
int onda_output_count(const onda_program_t* program);
/* Returns declared kouts/control-output entry count, or -1 on invalid program handle. */
int onda_control_output_count(const onda_program_t* program);
/* Returns declared parameter entry count, or -1 on invalid program handle. */
int onda_param_count(const onda_program_t* program);
/* Returns physical bindable buffer-slot count, or -1 on invalid program handle. */
int onda_buffer_count(const onda_program_t* program);
/* Returns declared buffer-array group count, or -1 on invalid program handle. */
int onda_buffer_array_count(const onda_program_t* program);
/* Returns declared event entry count, or -1 on invalid program handle. */
int onda_event_count(const onda_program_t* program);
/* Returns declared state entry count, or -1 on invalid program handle. */
int onda_state_count(const onda_program_t* program);

/* Returns input name by index, or NULL if index/program is invalid. */
const char* onda_input_name(const onda_program_t* program, int index);
/* Returns output name by index, or NULL if index/program is invalid. */
const char* onda_output_name(const onda_program_t* program, int index);
/* Returns kouts/control-output name by index, or NULL if index/program is invalid. */
const char* onda_control_output_name(const onda_program_t* program, int index);
/* Returns parameter name by index, or NULL if index/program is invalid. */
const char* onda_param_name(const onda_program_t* program, int index);
/* Returns physical buffer-slot name by index, or NULL if index/program is invalid. */
const char* onda_buffer_name(const onda_program_t* program, int index);
/* Returns buffer-array group metadata, or NULL/-1 if index/program is invalid. */
const char* onda_buffer_array_name(const onda_program_t* program, int index);
int onda_buffer_array_first(const onda_program_t* program, int index);
int onda_buffer_array_len(const onda_program_t* program, int index);
/* Returns event name by index, or NULL if index/program is invalid. */
const char* onda_event_name(const onda_program_t* program, int index);
/* Returns state entry name by index, or NULL if index/program is invalid. */
const char* onda_state_name(const onda_program_t* program, int index);
/* Returns event parameter count, or -1 if event/program is invalid. */
int onda_event_param_count(const onda_program_t* program, int event_index);
/* Returns event parameter name, or NULL if event/parameter/program is invalid. */
const char* onda_event_param_name(const onda_program_t* program, int event_index, int param_index);

/* Returns input index for a name, or -1 if not found/invalid. */
int onda_input_index(const onda_program_t* program, const char* name);
/* Returns output index for a name, or -1 if not found/invalid. */
int onda_output_index(const onda_program_t* program, const char* name);
/* Returns kouts/control-output index for a name, or -1 if not found/invalid. */
int onda_control_output_index(const onda_program_t* program, const char* name);
/* Returns parameter index for a name, or -1 if not found/invalid. */
int onda_param_index(const onda_program_t* program, const char* name);
/* Returns buffer index for a name, or -1 if not found/invalid. */
int onda_buffer_index(const onda_program_t* program, const char* name);
/* Returns event index for a name, or -1 if not found/invalid. */
int onda_event_index(const onda_program_t* program, const char* name);

/* Returns input type text (for example "f64[2]"), or NULL if invalid. */
const char* onda_input_type(const onda_program_t* program, int index);
/* Returns output type text (for example "f32"), or NULL if invalid. */
const char* onda_output_type(const onda_program_t* program, int index);
/* Returns kouts/control-output type text (for example "f32"), or NULL if invalid. */
const char* onda_control_output_type(const onda_program_t* program, int index);
/* Returns parameter type text, or NULL if invalid. */
const char* onda_param_type(const onda_program_t* program, int index);
/* Returns buffer type text (for example "buffer<f32[2]>"), or NULL if invalid. */
const char* onda_buffer_type(const onda_program_t* program, int index);
/* Returns state entry type text, or NULL if invalid. */
const char* onda_state_type(const onda_program_t* program, int index);

/* Returns input entry byte width, or -1 if invalid. */
int onda_input_type_bytes(const onda_program_t* program, int index);
/* Returns output entry byte width, or -1 if invalid. */
int onda_output_type_bytes(const onda_program_t* program, int index);
/* Returns kouts/control-output entry byte width, or -1 if invalid. */
int onda_control_output_type_bytes(const onda_program_t* program, int index);
/* Returns parameter entry byte width, or -1 if invalid. */
int onda_param_type_bytes(const onda_program_t* program, int index);
/* Returns state entry byte width, or -1 if invalid. */
int onda_state_type_bytes(const onda_program_t* program, int index);
/* Returns event payload byte width for fixed-shape events, or -1 if invalid or dynamic. */
int onda_event_payload_bytes(const onda_program_t* program, int index);
/* Returns event parameter element primitive type id, or -1 if invalid. */
int onda_event_param_elem_type(const onda_program_t* program, int event_index, int param_index);
/* Returns event parameter array length (1 for scalar, 0 for slice), or -1 if invalid. */
int onda_event_param_array_len(const onda_program_t* program, int event_index, int param_index);
/* Returns 1 if the event parameter is a slice, 0 if not, -1 if invalid. */
int onda_event_param_is_slice(const onda_program_t* program, int event_index, int param_index);
/* Returns event parameter byte offset within the packed payload, or -1 if invalid. */
int onda_event_param_offset_bytes(const onda_program_t* program, int event_index, int param_index);
/* Returns 1 if the event parameter has a default, 0 if not, -1 if invalid. */
int onda_event_param_has_default(const onda_program_t* program, int event_index, int param_index);
/*
 * Returns the byte count for the parameter default, 0 if no default exists, or -1 if invalid.
 * If out_bytes is non-NULL and out_capacity is large enough, the packed default bytes are copied.
 * If out_bytes is NULL or out_capacity is too small, no bytes are copied and the required size is returned.
 */
int onda_event_param_default_bytes(
  const onda_program_t* program,
  int event_index,
  int param_index,
  void* out_bytes,
  int out_capacity
);
/* Returns buffer element primitive type id, or -1 if invalid. */
int onda_buffer_elem_type(const onda_program_t* program, int index);
/* Returns buffer element byte width, or -1 if invalid. */
int onda_buffer_elem_type_bytes(const onda_program_t* program, int index);
/* Returns buffer channel kind (ONDA_BUFFER_CHANNELS_*), or -1 if invalid. */
int onda_buffer_channels_kind(const onda_program_t* program, int index);
/* Returns static channel count (mono=1), or -1 for dynamic/invalid. */
int onda_buffer_channels_static(const onda_program_t* program, int index);
/* Returns 1 if reachable program code may write the physical buffer slot,
   including when a collection selector cannot be resolved statically. Returns
   0 only when no reachable write is possible, or -1 for an invalid program/index. */
int onda_buffer_may_write(const onda_program_t* program, int index);

/* Returns input element primitive type id, or -1 if invalid. */
int onda_input_elem_type(const onda_program_t* program, int index);
/* Returns output element primitive type id, or -1 if invalid. */
int onda_output_elem_type(const onda_program_t* program, int index);
/* Returns kouts/control-output element primitive type id, or -1 if invalid. */
int onda_control_output_elem_type(const onda_program_t* program, int index);
/* Returns parameter element primitive type id, or -1 if invalid. */
int onda_param_elem_type(const onda_program_t* program, int index);
/* Returns state entry element primitive type id, or -1 if invalid. */
int onda_state_elem_type(const onda_program_t* program, int index);
/* Returns input array length in channels/slots, or -1 if invalid. */
int onda_input_array_len(const onda_program_t* program, int index);
/* Returns output array length in channels/slots, or -1 if invalid. */
int onda_output_array_len(const onda_program_t* program, int index);
/* Returns kouts/control-output array length in slots, or -1 if invalid. */
int onda_control_output_array_len(const onda_program_t* program, int index);
/* Returns parameter array length in slots, or -1 if invalid. */
int onda_param_array_len(const onda_program_t* program, int index);
/* Returns state entry array length in slots, or -1 if invalid. */
int onda_state_array_len(const onda_program_t* program, int index);
/* Returns input slot offset in flattened channel order, or -1 if invalid. */
int onda_input_slot_offset(const onda_program_t* program, int index);
/* Returns output slot offset in flattened channel order, or -1 if invalid. */
int onda_output_slot_offset(const onda_program_t* program, int index);
/* Returns kouts/control-output slot offset in flattened control-output order, or -1 if invalid. */
int onda_control_output_slot_offset(const onda_program_t* program, int index);
/* Returns parameter slot offset in flattened param order, or -1 if invalid. */
int onda_param_slot_offset(const onda_program_t* program, int index);
/* Returns input byte offset within packed entry layout, or -1 if invalid. */
int onda_input_byte_offset(const onda_program_t* program, int index);
/* Returns output byte offset within packed entry layout, or -1 if invalid. */
int onda_output_byte_offset(const onda_program_t* program, int index);
/* Returns kouts/control-output byte offset within packed entry layout, or -1 if invalid. */
int onda_control_output_byte_offset(const onda_program_t* program, int index);
/* Returns parameter byte offset within packed param layout, or -1 if invalid. */
int onda_param_byte_offset(const onda_program_t* program, int index);
/* Returns state entry byte offset within the packed persistent snapshot, or -1 if invalid. */
int onda_state_byte_offset(const onda_program_t* program, int index);
/* Returns the packed persistent snapshot byte size for this program, or -1 if invalid. */
int onda_state_total_bytes(const onda_program_t* program);

/* Returns 1 if input default exists, 0 if not, -1 if invalid. */
int onda_input_has_default(const onda_program_t* program, int index);
/* Returns 1 if output default exists, 0 if not, -1 if invalid. */
int onda_output_has_default(const onda_program_t* program, int index);
/* Returns 1 if parameter default exists, 0 if not, -1 if invalid. */
int onda_param_has_default(const onda_program_t* program, int index);
/*
 * Returns the byte count for the parameter default, 0 if no default exists, or -1 if invalid.
 * If out_bytes is non-NULL and out_capacity is large enough, the packed default bytes are copied.
 * If out_bytes is NULL or out_capacity is too small, no bytes are copied and the required size is returned.
 */
int onda_param_default_bytes(
  const onda_program_t* program,
  int index,
  void* out_bytes,
  int out_capacity
);
/* Returns input default as f64, or NaN if missing/invalid. */
double onda_input_default_f64(const onda_program_t* program, int index);
/* Returns output default as f64, or NaN if missing/invalid. */
double onda_output_default_f64(const onda_program_t* program, int index);
/* Returns parameter default as f64, or NaN if missing/invalid. */
double onda_param_default_f64(const onda_program_t* program, int index);

/* Returns 1 if input range exists, 0 if not, -1 if invalid. */
int onda_input_has_range(const onda_program_t* program, int index);
/* Returns 1 if output range exists, 0 if not, -1 if invalid. */
int onda_output_has_range(const onda_program_t* program, int index);
/* Returns 1 if parameter range exists, 0 if not, -1 if invalid. */
int onda_param_has_range(const onda_program_t* program, int index);
/* Returns input range minimum as f64, or NaN if missing/invalid. */
double onda_input_range_min_f64(const onda_program_t* program, int index);
/* Returns input range maximum as f64, or NaN if missing/invalid. */
double onda_input_range_max_f64(const onda_program_t* program, int index);
/* Returns output range minimum as f64, or NaN if missing/invalid. */
double onda_output_range_min_f64(const onda_program_t* program, int index);
/* Returns output range maximum as f64, or NaN if missing/invalid. */
double onda_output_range_max_f64(const onda_program_t* program, int index);
/* Returns parameter range minimum as f64, or NaN if missing/invalid. */
double onda_param_range_min_f64(const onda_program_t* program, int index);
/* Returns parameter range maximum as f64, or NaN if missing/invalid. */
double onda_param_range_max_f64(const onda_program_t* program, int index);
/* Returns ONDA_PARAM_SCALE_*, or -1 if the index is invalid/non-scalar. */
int onda_param_scale(const onda_program_t* program, int index);
/* Returns 1 when the parameter has lincurve shaping, 0 when absent, -1 if invalid. */
int onda_param_has_curve(const onda_program_t* program, int index);
/* Returns the finite lincurve value, or NaN when absent/invalid. */
double onda_param_curve(const onda_program_t* program, int index);
/* Copies the UTF-8 unit including its trailing NUL. Returns 0 when absent, the
   required byte count when present, or -1 on invalid arguments. */
int onda_param_unit_copy(
  const onda_program_t* program,
  int index,
  char* out_bytes,
  int out_capacity
);
/* Returns 1 when the parameter has a discrete step, 0 when continuous, -1 if invalid. */
int onda_param_has_step(const onda_program_t* program, int index);
double onda_param_step_f64(const onda_program_t* program, int index);
/* Number of intervals between min and max; returns 0 when absent/invalid. */
uint32_t onda_param_step_count(const onda_program_t* program, int index);
double onda_param_normalized_to_plain(
  const onda_program_t* program,
  int index,
  double normalized
);
double onda_param_plain_to_normalized(
  const onda_program_t* program,
  int index,
  double plain
);

#ifdef __cplusplus
}
#endif

#endif
