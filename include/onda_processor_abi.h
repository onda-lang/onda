#ifndef ONDA_PROCESSOR_ABI_H
#define ONDA_PROCESSOR_ABI_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define ONDA_PROCESSOR_ABI_VERSION 1u

typedef void (*onda_processor_init_fn)(const void* params, void* state);

typedef void (*onda_processor_process_fn)(
  void* state,
  const void* params,
  const void* const* inputs,
  void* const* outputs,
  int32_t start_frame,
  int32_t frames,
  int32_t flags,
  void* const* buffers,
  const int32_t* buffer_frames,
  const int32_t* buffer_channels,
  const float* buffer_sample_rates
);

typedef void (*onda_processor_event_fn)(
  const void* payload,
  const void* params,
  void* state,
  void* const* buffers,
  const int32_t* buffer_frames,
  const int32_t* buffer_channels,
  const float* buffer_sample_rates
);

enum {
  ONDA_PROCESSOR_BEGIN_BLOCK = 1u << 0,
  ONDA_PROCESSOR_END_BLOCK = 1u << 1,
  ONDA_PROCESSOR_FULL_BLOCK = ONDA_PROCESSOR_BEGIN_BLOCK | ONDA_PROCESSOR_END_BLOCK
};

/*
 * ABI-v1 symbols emitted by every native processor object. Pointer tables and
 * storage pointers are NULL exactly when the paired descriptor reports that
 * surface count or storage size as zero.
 */
void onda_init(const void* params, void* state);

void onda_process(
  void* state,
  const void* params,
  const void* const* inputs,
  void* const* outputs,
  int32_t start_frame,
  int32_t frames,
  int32_t flags,
  void* const* buffers,
  const int32_t* buffer_frames,
  const int32_t* buffer_channels,
  const float* buffer_sample_rates
);

/* Event symbols are named onda_event_N in descriptor metadata order. */

#ifdef __cplusplus
}
#endif

#endif
