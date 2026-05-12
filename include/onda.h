#ifndef ONDA_H
#define ONDA_H

#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct onda_program onda_program_t;
typedef struct onda_instance onda_instance_t;

/* Primitive element type identifiers used by metadata and buffer binding APIs. */
enum {
  ONDA_PRIMITIVE_F32 = 0,
  ONDA_PRIMITIVE_F64 = 1,
  ONDA_PRIMITIVE_I32 = 2,
  ONDA_PRIMITIVE_I64 = 3,
  ONDA_PRIMITIVE_BOOL = 4
};

/* Buffer channel declaration kinds used by buffer metadata queries. */
enum {
  ONDA_BUFFER_CHANNELS_MONO = 0,
  ONDA_BUFFER_CHANNELS_STATIC = 1,
  ONDA_BUFFER_CHANNELS_DYNAMIC = 2
};

/* Diagnostic payload returned from compile/create failures. */
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

/* Compile options for onda_compile. */
typedef struct {
  /* fast_math != 0 enables LLVM fast-math lowering. */
  int fast_math;
  /* Compile-time sample rate constant. Must be finite and > 0. */
  float sample_rate;
  /* Fixed compile-time block size. Must be > 0. */
  int block_size;
} onda_compile_options_t;

typedef void* (*onda_alloc_fn)(void* context, size_t size, size_t align);
typedef void (*onda_free_fn)(void* context, void* ptr, size_t size, size_t align);

/* Host allocator used by custom instance creation. */
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
/* Compiles an Onda file and returns a program handle, or NULL on failure.
   Relative include/import resolution uses file_path_utf8 as the entry path. */
onda_program_t* onda_compile_file(
  const char* file_path_utf8,
  const onda_compile_options_t* options,
  onda_diag_t* out_diag
);
/* Destroys a program handle created by onda_compile. */
void onda_program_destroy(onda_program_t* program);

/* Creates a runtime instance for a compiled program, or NULL on failure.
   Uses compile-time sample_rate and block_size captured in the program handle. */
onda_instance_t* onda_instance_create(
  const onda_program_t* program,
  int in_channels,
  int out_channels,
  onda_diag_t* out_diag
);
/* Creates a runtime instance whose instance-owned storage uses allocator.
   The returned instance still retains a reference to the compiled program internally;
   compiled/JIT memory is released when the last program/instance reference is gone.
   allocator->alloc and allocator->free must be non-NULL and must honor size/align. */
onda_instance_t* onda_instance_create_with_allocator(
  const onda_program_t* program,
  int in_channels,
  int out_channels,
  const onda_allocator_t* allocator,
  onda_diag_t* out_diag
);
/* Destroys an instance handle created by onda_instance_create or onda_instance_create_with_allocator. */
void onda_instance_destroy(onda_instance_t* instance);

/* Sets a parameter by index from raw bytes; returns 0 on success, negative on error. */
int onda_set_param_by_index(
  onda_instance_t* instance,
  int index,
  const void* value_ptr,
  int value_bytes
);

/* Triggers one event by index with packed payload bytes; returns 0 on success, negative on error.
   Unknown event indices are ignored and return success. */
int onda_trigger_event_by_index(
  onda_instance_t* instance,
  int index,
  const void* payload_ptr,
  int payload_bytes
);
/* Triggers one event without payload/binding validation; unsafe if payload/buffer metadata is invalid. */
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
   src_ptr memory must be readable during processing.
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
   dst_ptr memory must be writable during processing.
   Contract for optimized codegen: bound input/output/buffer memory regions must not overlap. */
int onda_bind_output(
  onda_instance_t* instance,
  int index,
  void* dst_ptr,
  int dst_bytes
);

/* Binds one buffer entry; elem_type must be an ONDA_PRIMITIVE_* value.
   Zero-copy contract: runtime stores ptr and accesses it directly (no internal copy).
   ptr must remain valid, correctly sized for the declared shape, and at a stable address until
   this slot is rebound/unbound (null + 0 frames + 0 channels) or the instance is destroyed.
   ptr memory must be writable during processing.
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

enum {
  ONDA_PROCESS_BEGIN_BLOCK = 1 << 0,
  ONDA_PROCESS_END_BLOCK = 1 << 1,
  ONDA_PROCESS_FULL_BLOCK = ONDA_PROCESS_BEGIN_BLOCK | ONDA_PROCESS_END_BLOCK
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
 * Copies the full instance state snapshot.
 * If out_bytes is NULL or out_capacity is too small, no bytes are copied and the required size is returned.
 */
int onda_instance_snapshot_state(
  const onda_instance_t* instance,
  void* out_bytes,
  int out_capacity
);
/* Restores a full state snapshot copied from a compatible program/instance; returns 0 on success. */
int onda_instance_restore_state(
  onda_instance_t* instance,
  const void* bytes,
  int byte_count
);
/* Validates all required domains (buffers, inputs, outputs); returns 0 on success. */
int onda_validate_bindings(onda_instance_t* instance);
/* Validates input bindings only; returns 0 on success. */
int onda_validate_inputs(onda_instance_t* instance);
/* Validates output bindings only; returns 0 on success. */
int onda_validate_outputs(onda_instance_t* instance);
/* Validates buffer bindings only; returns 0 on success. */
int onda_validate_buffers(onda_instance_t* instance);
/* Validates all bindings and refreshes proc-slot buffer refs before unchecked processing. */
int onda_prepare_unchecked_process(onda_instance_t* instance);
/* Processes a full logical block without revalidation (unsafe if bindings are stale);
   returns 0 on success. */
int onda_process_unchecked(onda_instance_t* instance);
/* Processes one logical-block segment without revalidation. Use the same full-block
   binding, start_frame, frames, and flags contract as onda_process_checked_segment. */
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
/* Returns declared parameter entry count, or -1 on invalid program handle. */
int onda_param_count(const onda_program_t* program);
/* Returns declared buffer entry count, or -1 on invalid program handle. */
int onda_buffer_count(const onda_program_t* program);
/* Returns declared event entry count, or -1 on invalid program handle. */
int onda_event_count(const onda_program_t* program);
/* Returns declared state entry count, or -1 on invalid program handle. */
int onda_state_count(const onda_program_t* program);

/* Returns input name by index, or NULL if index/program is invalid. */
const char* onda_input_name(const onda_program_t* program, int index);
/* Returns output name by index, or NULL if index/program is invalid. */
const char* onda_output_name(const onda_program_t* program, int index);
/* Returns parameter name by index, or NULL if index/program is invalid. */
const char* onda_param_name(const onda_program_t* program, int index);
/* Returns buffer name by index, or NULL if index/program is invalid. */
const char* onda_buffer_name(const onda_program_t* program, int index);
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
/* Returns parameter type text, or NULL if invalid. */
const char* onda_param_type(const onda_program_t* program, int index);
/* Returns buffer type text (for example "buffer[f32[2]]"), or NULL if invalid. */
const char* onda_buffer_type(const onda_program_t* program, int index);
/* Returns state entry type text, or NULL if invalid. */
const char* onda_state_type(const onda_program_t* program, int index);

/* Returns input entry byte width, or -1 if invalid. */
int onda_input_type_bytes(const onda_program_t* program, int index);
/* Returns output entry byte width, or -1 if invalid. */
int onda_output_type_bytes(const onda_program_t* program, int index);
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
/* Returns 1 if buffer may be written by program code, 0 if read-only, -1 if invalid. */
int onda_buffer_may_write(const onda_program_t* program, int index);

/* Returns input element primitive type id, or -1 if invalid. */
int onda_input_elem_type(const onda_program_t* program, int index);
/* Returns output element primitive type id, or -1 if invalid. */
int onda_output_elem_type(const onda_program_t* program, int index);
/* Returns parameter element primitive type id, or -1 if invalid. */
int onda_param_elem_type(const onda_program_t* program, int index);
/* Returns state entry element primitive type id, or -1 if invalid. */
int onda_state_elem_type(const onda_program_t* program, int index);
/* Returns input array length in channels/slots, or -1 if invalid. */
int onda_input_array_len(const onda_program_t* program, int index);
/* Returns output array length in channels/slots, or -1 if invalid. */
int onda_output_array_len(const onda_program_t* program, int index);
/* Returns parameter array length in slots, or -1 if invalid. */
int onda_param_array_len(const onda_program_t* program, int index);
/* Returns state entry array length in slots, or -1 if invalid. */
int onda_state_array_len(const onda_program_t* program, int index);
/* Returns input slot offset in flattened channel order, or -1 if invalid. */
int onda_input_slot_offset(const onda_program_t* program, int index);
/* Returns output slot offset in flattened channel order, or -1 if invalid. */
int onda_output_slot_offset(const onda_program_t* program, int index);
/* Returns parameter slot offset in flattened param order, or -1 if invalid. */
int onda_param_slot_offset(const onda_program_t* program, int index);
/* Returns input byte offset within packed entry layout, or -1 if invalid. */
int onda_input_byte_offset(const onda_program_t* program, int index);
/* Returns output byte offset within packed entry layout, or -1 if invalid. */
int onda_output_byte_offset(const onda_program_t* program, int index);
/* Returns parameter byte offset within packed param layout, or -1 if invalid. */
int onda_param_byte_offset(const onda_program_t* program, int index);
/* Returns state entry byte offset within the full instance state layout, or -1 if invalid. */
int onda_state_byte_offset(const onda_program_t* program, int index);
/* Returns total instance state byte size for this program, or -1 if invalid. */
int onda_state_total_bytes(const onda_program_t* program);

/* Returns 1 if input default exists, 0 if not, -1 if invalid. */
int onda_input_has_default(const onda_program_t* program, int index);
/* Returns 1 if output default exists, 0 if not, -1 if invalid. */
int onda_output_has_default(const onda_program_t* program, int index);
/* Returns 1 if parameter default exists, 0 if not, -1 if invalid. */
int onda_param_has_default(const onda_program_t* program, int index);
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

#ifdef __cplusplus
}
#endif

#endif
