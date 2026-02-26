#ifndef OMNI_LLVM_H
#define OMNI_LLVM_H

#ifdef __cplusplus
extern "C" {
#endif

typedef struct omni_program omni_program_t;
typedef struct omni_instance omni_instance_t;

/* Primitive element type identifiers used by metadata and buffer binding APIs. */
enum {
  OMNI_PRIMITIVE_F32 = 0,
  OMNI_PRIMITIVE_F64 = 1,
  OMNI_PRIMITIVE_I32 = 2,
  OMNI_PRIMITIVE_I64 = 3,
  OMNI_PRIMITIVE_BOOL = 4
};

/* Buffer channel declaration kinds used by buffer metadata queries. */
enum {
  OMNI_BUFFER_CHANNELS_MONO = 0,
  OMNI_BUFFER_CHANNELS_STATIC = 1,
  OMNI_BUFFER_CHANNELS_DYNAMIC = 2
};

/* Diagnostic payload returned from compile/create failures. */
typedef struct {
  int code;
  int line;
  int column;
  const char* message;
  const char* file;
  const char* trace;
} omni_diag_t;

/* Compiles Omni source and returns a program handle, or NULL on failure.
   fast_math != 0 enables LLVM fast-math lowering. */
omni_program_t* omni_compile(
  const char* src_utf8,
  int fast_math,
  omni_diag_t* out_diag
);
/* Destroys a program handle created by omni_compile. */
void omni_program_destroy(omni_program_t* program);

/* Creates a runtime instance for a compiled program, or NULL on failure. */
omni_instance_t* omni_instance_create(
  const omni_program_t* program,
  float sample_rate,
  int frames_per_block,
  int in_channels,
  int out_channels,
  omni_diag_t* out_diag
);
/* Destroys an instance handle created by omni_instance_create. */
void omni_instance_destroy(omni_instance_t* instance);

/* Sets a parameter by index from raw bytes; returns 0 on success, negative on error. */
int omni_set_param_by_index(
  omni_instance_t* instance,
  int index,
  const void* value_ptr,
  int value_bytes
);

/* Triggers one event by index with packed payload bytes; returns 0 on success, negative on error.
   Unknown event indices are ignored and return success. */
int omni_trigger_event_by_index(
  omni_instance_t* instance,
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
int omni_bind_input(
  omni_instance_t* instance,
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
int omni_bind_output(
  omni_instance_t* instance,
  int index,
  void* dst_ptr,
  int dst_bytes
);

/* Binds one buffer entry; elem_type must be an OMNI_PRIMITIVE_* value.
   Zero-copy contract: runtime stores ptr and accesses it directly (no internal copy).
   ptr must remain valid, correctly sized for the declared shape, and at a stable address until
   this slot is rebound/unbound (null + 0 frames + 0 channels) or the instance is destroyed.
   ptr memory must be writable during processing.
   Contract for optimized codegen: bound input/output/buffer memory regions must not overlap. */
int omni_bind_buffer(
  omni_instance_t* instance,
  int index,
  void* ptr,
  int frames,
  int channels,
  int elem_type
);

/* Processes one block with current bindings and parameters; returns 0 on success. */
int omni_process_bound(omni_instance_t* instance, int frames);
/* Resets instance DSP/state memory to the initial post-init state captured at instance creation. */
int omni_reset_instance_state(omni_instance_t* instance);
/* Validates all required domains (buffers, inputs, outputs); returns 0 on success. */
int omni_validate_bindings(omni_instance_t* instance);
/* Validates input bindings only; returns 0 on success. */
int omni_validate_inputs(omni_instance_t* instance);
/* Validates output bindings only; returns 0 on success. */
int omni_validate_outputs(omni_instance_t* instance);
/* Validates buffer bindings only; returns 0 on success. */
int omni_validate_buffers(omni_instance_t* instance);
/* Processes without revalidation (unsafe if bindings are stale); returns 0 on success. */
int omni_process_unchecked(omni_instance_t* instance);

/* Returns declared input entry count, or -1 on invalid program handle. */
int omni_input_count(const omni_program_t* program);
/* Returns declared output entry count, or -1 on invalid program handle. */
int omni_output_count(const omni_program_t* program);
/* Returns declared parameter entry count, or -1 on invalid program handle. */
int omni_param_count(const omni_program_t* program);
/* Returns declared buffer entry count, or -1 on invalid program handle. */
int omni_buffer_count(const omni_program_t* program);
/* Returns declared event entry count, or -1 on invalid program handle. */
int omni_event_count(const omni_program_t* program);

/* Returns input name by index, or NULL if index/program is invalid. */
const char* omni_input_name(const omni_program_t* program, int index);
/* Returns output name by index, or NULL if index/program is invalid. */
const char* omni_output_name(const omni_program_t* program, int index);
/* Returns parameter name by index, or NULL if index/program is invalid. */
const char* omni_param_name(const omni_program_t* program, int index);
/* Returns buffer name by index, or NULL if index/program is invalid. */
const char* omni_buffer_name(const omni_program_t* program, int index);
/* Returns event name by index, or NULL if index/program is invalid. */
const char* omni_event_name(const omni_program_t* program, int index);

/* Returns input index for a name, or -1 if not found/invalid. */
int omni_input_index(const omni_program_t* program, const char* name);
/* Returns output index for a name, or -1 if not found/invalid. */
int omni_output_index(const omni_program_t* program, const char* name);
/* Returns parameter index for a name, or -1 if not found/invalid. */
int omni_param_index(const omni_program_t* program, const char* name);
/* Returns buffer index for a name, or -1 if not found/invalid. */
int omni_buffer_index(const omni_program_t* program, const char* name);
/* Returns event index for a name, or -1 if not found/invalid. */
int omni_event_index(const omni_program_t* program, const char* name);

/* Returns input type text (for example "f64[2]"), or NULL if invalid. */
const char* omni_input_type(const omni_program_t* program, int index);
/* Returns output type text (for example "f32"), or NULL if invalid. */
const char* omni_output_type(const omni_program_t* program, int index);
/* Returns parameter type text, or NULL if invalid. */
const char* omni_param_type(const omni_program_t* program, int index);
/* Returns buffer type text (for example "buffer[f32[2]]"), or NULL if invalid. */
const char* omni_buffer_type(const omni_program_t* program, int index);

/* Returns input entry byte width, or -1 if invalid. */
int omni_input_type_bytes(const omni_program_t* program, int index);
/* Returns output entry byte width, or -1 if invalid. */
int omni_output_type_bytes(const omni_program_t* program, int index);
/* Returns parameter entry byte width, or -1 if invalid. */
int omni_param_type_bytes(const omni_program_t* program, int index);
/* Returns event payload byte width, or -1 if invalid. */
int omni_event_payload_bytes(const omni_program_t* program, int index);
/* Returns buffer element primitive type id, or -1 if invalid. */
int omni_buffer_elem_type(const omni_program_t* program, int index);
/* Returns buffer element byte width, or -1 if invalid. */
int omni_buffer_elem_type_bytes(const omni_program_t* program, int index);
/* Returns buffer channel kind (OMNI_BUFFER_CHANNELS_*), or -1 if invalid. */
int omni_buffer_channels_kind(const omni_program_t* program, int index);
/* Returns static channel count (mono=1), or -1 for dynamic/invalid. */
int omni_buffer_channels_static(const omni_program_t* program, int index);

/* Returns input element primitive type id, or -1 if invalid. */
int omni_input_elem_type(const omni_program_t* program, int index);
/* Returns output element primitive type id, or -1 if invalid. */
int omni_output_elem_type(const omni_program_t* program, int index);
/* Returns parameter element primitive type id, or -1 if invalid. */
int omni_param_elem_type(const omni_program_t* program, int index);
/* Returns input array length in channels/slots, or -1 if invalid. */
int omni_input_array_len(const omni_program_t* program, int index);
/* Returns output array length in channels/slots, or -1 if invalid. */
int omni_output_array_len(const omni_program_t* program, int index);
/* Returns parameter array length in slots, or -1 if invalid. */
int omni_param_array_len(const omni_program_t* program, int index);
/* Returns input slot offset in flattened channel order, or -1 if invalid. */
int omni_input_slot_offset(const omni_program_t* program, int index);
/* Returns output slot offset in flattened channel order, or -1 if invalid. */
int omni_output_slot_offset(const omni_program_t* program, int index);
/* Returns parameter slot offset in flattened param order, or -1 if invalid. */
int omni_param_slot_offset(const omni_program_t* program, int index);
/* Returns input byte offset within packed entry layout, or -1 if invalid. */
int omni_input_byte_offset(const omni_program_t* program, int index);
/* Returns output byte offset within packed entry layout, or -1 if invalid. */
int omni_output_byte_offset(const omni_program_t* program, int index);
/* Returns parameter byte offset within packed param layout, or -1 if invalid. */
int omni_param_byte_offset(const omni_program_t* program, int index);

/* Returns 1 if input default exists, 0 if not, -1 if invalid. */
int omni_input_has_default(const omni_program_t* program, int index);
/* Returns 1 if output default exists, 0 if not, -1 if invalid. */
int omni_output_has_default(const omni_program_t* program, int index);
/* Returns 1 if parameter default exists, 0 if not, -1 if invalid. */
int omni_param_has_default(const omni_program_t* program, int index);
/* Returns input default as f64, or NaN if missing/invalid. */
double omni_input_default_f64(const omni_program_t* program, int index);
/* Returns output default as f64, or NaN if missing/invalid. */
double omni_output_default_f64(const omni_program_t* program, int index);
/* Returns parameter default as f64, or NaN if missing/invalid. */
double omni_param_default_f64(const omni_program_t* program, int index);

/* Returns 1 if input range exists, 0 if not, -1 if invalid. */
int omni_input_has_range(const omni_program_t* program, int index);
/* Returns 1 if output range exists, 0 if not, -1 if invalid. */
int omni_output_has_range(const omni_program_t* program, int index);
/* Returns 1 if parameter range exists, 0 if not, -1 if invalid. */
int omni_param_has_range(const omni_program_t* program, int index);
/* Returns input range minimum as f64, or NaN if missing/invalid. */
double omni_input_range_min_f64(const omni_program_t* program, int index);
/* Returns input range maximum as f64, or NaN if missing/invalid. */
double omni_input_range_max_f64(const omni_program_t* program, int index);
/* Returns output range minimum as f64, or NaN if missing/invalid. */
double omni_output_range_min_f64(const omni_program_t* program, int index);
/* Returns output range maximum as f64, or NaN if missing/invalid. */
double omni_output_range_max_f64(const omni_program_t* program, int index);
/* Returns parameter range minimum as f64, or NaN if missing/invalid. */
double omni_param_range_min_f64(const omni_program_t* program, int index);
/* Returns parameter range maximum as f64, or NaN if missing/invalid. */
double omni_param_range_max_f64(const omni_program_t* program, int index);

#ifdef __cplusplus
}
#endif

#endif
