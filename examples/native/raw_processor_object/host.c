#include <math.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "onda_processor_abi.h"
#include "processor_config.h"

#if PROCESSOR_DESCRIPTOR_ABI_VERSION != ONDA_PROCESSOR_ABI_VERSION
#error "generated descriptor and C ABI header disagree"
#endif

static int pointer_meets_alignment(const void* storage, size_t alignment) {
  return storage == NULL || (alignment != 0 && (uintptr_t)storage % alignment == 0);
}

static int execution_succeeded(uint32_t execution_status, const char* operation) {
  if (execution_status == ONDA_PROCESSOR_EXECUTION_OK) {
    return 1;
  }
  fprintf(stderr, "%s failed with Onda execution status %u\n", operation, execution_status);
  return 0;
}

static size_t scalar_size(unsigned char kind) {
  switch (kind) {
    case PROCESSOR_SCALAR_BOOL:
      return sizeof(uint8_t);
    case PROCESSOR_SCALAR_F32:
      return sizeof(float);
    case PROCESSOR_SCALAR_F64:
      return sizeof(double);
    case PROCESSOR_SCALAR_I32:
      return sizeof(int32_t);
    case PROCESSOR_SCALAR_I64:
      return sizeof(int64_t);
    default:
      return 0;
  }
}

static void write_scalar(void* storage, size_t index, unsigned char kind, double value) {
  switch (kind) {
    case PROCESSOR_SCALAR_BOOL:
      ((uint8_t*)storage)[index] = value != 0.0;
      break;
    case PROCESSOR_SCALAR_F32:
      ((float*)storage)[index] = (float)value;
      break;
    case PROCESSOR_SCALAR_F64:
      ((double*)storage)[index] = value;
      break;
    case PROCESSOR_SCALAR_I32:
      ((int32_t*)storage)[index] = (int32_t)value;
      break;
    case PROCESSOR_SCALAR_I64:
      ((int64_t*)storage)[index] = (int64_t)value;
      break;
  }
}

static double scalar_abs(const void* storage, size_t index, unsigned char kind) {
  switch (kind) {
    case PROCESSOR_SCALAR_BOOL:
      return ((const uint8_t*)storage)[index] == 0 ? 0.0 : 1.0;
    case PROCESSOR_SCALAR_F32:
      return fabs((double)((const float*)storage)[index]);
    case PROCESSOR_SCALAR_F64:
      return fabs(((const double*)storage)[index]);
    case PROCESSOR_SCALAR_I32:
      return fabs((double)((const int32_t*)storage)[index]);
    case PROCESSOR_SCALAR_I64:
      return fabs((double)((const int64_t*)storage)[index]);
    default:
      return 0.0;
  }
}

static double peak(const void* storage, unsigned char kind) {
  double result = 0.0;
  for (int frame = 0; frame < PROCESSOR_BLOCK_SIZE; ++frame) {
    result = fmax(result, scalar_abs(storage, (size_t)frame, kind));
  }
  return result;
}

static void fill_buffer(
  void* storage,
  unsigned char kind,
  int32_t frames,
  int32_t channels
) {
  for (int32_t frame = 0; frame < frames; ++frame) {
    const double ramp = (double)(frame % 32) / 31.0;
    for (int32_t channel = 0; channel < channels; ++channel) {
      const double value = channel % 2 == 0 ? ramp : -ramp;
      const size_t index = (size_t)frame * (size_t)channels + (size_t)channel;
      write_scalar(storage, index, kind, value);
    }
  }
}

int main(void) {
  int status = 1;
  void* state = PROCESSOR_STATE_SIZE == 0 ? NULL : malloc(PROCESSOR_STATE_SIZE);
  void* params = PROCESSOR_PARAM_SIZE == 0 ? NULL : malloc(PROCESSOR_PARAM_SIZE);
  const void** inputs = PROCESSOR_INPUT_COUNT == 0
    ? NULL
    : calloc(PROCESSOR_INPUT_COUNT, sizeof(*inputs));
  void** outputs = PROCESSOR_OUTPUT_COUNT == 0
    ? NULL
    : calloc(PROCESSOR_OUTPUT_COUNT, sizeof(*outputs));
  void** buffers = PROCESSOR_BUFFER_COUNT == 0
    ? NULL
    : calloc(PROCESSOR_BUFFER_COUNT, sizeof(*buffers));
  int32_t* buffer_frames = PROCESSOR_BUFFER_COUNT == 0
    ? NULL
    : calloc(PROCESSOR_BUFFER_COUNT, sizeof(*buffer_frames));
  int32_t* buffer_channels = PROCESSOR_BUFFER_COUNT == 0
    ? NULL
    : calloc(PROCESSOR_BUFFER_COUNT, sizeof(*buffer_channels));
  float* buffer_sample_rates = PROCESSOR_BUFFER_COUNT == 0
    ? NULL
    : calloc(PROCESSOR_BUFFER_COUNT, sizeof(*buffer_sample_rates));

  if (
    (PROCESSOR_STATE_SIZE > 0 && state == NULL) ||
    (PROCESSOR_PARAM_SIZE > 0 && params == NULL) ||
    (PROCESSOR_INPUT_COUNT > 0 && inputs == NULL) ||
    (PROCESSOR_OUTPUT_COUNT > 0 && outputs == NULL) ||
    (PROCESSOR_BUFFER_COUNT > 0 && (
      buffers == NULL || buffer_frames == NULL || buffer_channels == NULL ||
      buffer_sample_rates == NULL
    ))
  ) {
    fputs("could not allocate processor integration storage\n", stderr);
    goto cleanup;
  }

  if (
    !pointer_meets_alignment(state, PROCESSOR_STATE_ALIGN) ||
    !pointer_meets_alignment(params, PROCESSOR_PARAM_ALIGN)
  ) {
    fputs("malloc did not satisfy the processor storage alignment\n", stderr);
    goto cleanup;
  }

  if (state != NULL) {
    memset(state, 0, PROCESSOR_STATE_SIZE);
  }
  if (params != NULL) {
    memcpy(params, PROCESSOR_PARAM_DEFAULT_BYTES, PROCESSOR_PARAM_SIZE);
  }

  for (int index = 0; index < PROCESSOR_PARAM_COUNT; ++index) {
    const double plain = processor_param_read_plain(params, index);
    const double normalized = processor_param_plain_to_normalized(index, plain);
    if (isnan(normalized)) {
      printf("parameter[%d] '%s': not a scalar host control\n", index, PROCESSOR_PARAM_NAMES[index]);
      continue;
    }
    if (processor_param_set_normalized(params, index, normalized) != 0) {
      fprintf(stderr, "could not restore normalized parameter[%d]\n", index);
      goto cleanup;
    }
    printf(
      "parameter[%d] '%s': plain %.6f%s%s, normalized %.6f\n",
      index,
      PROCESSOR_PARAM_NAMES[index],
      processor_param_read_plain(params, index),
      PROCESSOR_PARAM_UNITS[index] == NULL ? "" : " ",
      PROCESSOR_PARAM_UNITS[index] == NULL ? "" : PROCESSOR_PARAM_UNITS[index],
      normalized
    );
  }

  for (int slot = 0; slot < PROCESSOR_INPUT_COUNT; ++slot) {
    const unsigned char kind = PROCESSOR_INPUT_KINDS[slot];
    const size_t bytes = scalar_size(kind) * (size_t)PROCESSOR_BLOCK_SIZE;
    void* storage = calloc(1, bytes);
    if (storage == NULL) {
      goto cleanup;
    }
    inputs[slot] = storage;
    for (int frame = 0; frame < PROCESSOR_BLOCK_SIZE; ++frame) {
      write_scalar(storage, (size_t)frame, kind, 0.2);
    }
  }

  for (int slot = 0; slot < PROCESSOR_OUTPUT_COUNT; ++slot) {
    const unsigned char kind = PROCESSOR_OUTPUT_KINDS[slot];
    const size_t bytes = scalar_size(kind) * (size_t)PROCESSOR_BLOCK_SIZE;
    outputs[slot] = calloc(1, bytes);
    if (outputs[slot] == NULL) {
      goto cleanup;
    }
  }

  for (int index = 0; index < PROCESSOR_BUFFER_COUNT; ++index) {
    const unsigned char kind = PROCESSOR_BUFFER_KINDS[index];
    const int32_t frames = PROCESSOR_BLOCK_SIZE;
    const int32_t channels = PROCESSOR_BUFFER_CHANNELS[index];
    const size_t elements = (size_t)frames * (size_t)channels;
    void* storage = calloc(elements, scalar_size(kind));
    if (storage == NULL) {
      goto cleanup;
    }
    buffers[index] = storage;
    buffer_frames[index] = frames;
    buffer_channels[index] = channels;
    buffer_sample_rates[index] = PROCESSOR_SAMPLE_RATE;
    fill_buffer(storage, kind, frames, channels);
    printf(
      "bound buffer[%d] '%s': %d frames, %d channels, %.0f Hz\n",
      index,
      PROCESSOR_BUFFER_NAMES[index],
      frames,
      channels,
      buffer_sample_rates[index]
    );
  }

  if (!execution_succeeded(onda_init(params, state), "processor init")) {
    goto cleanup;
  }

  for (int index = 0; index < PROCESSOR_EVENT_COUNT; ++index) {
    if (!PROCESSOR_EVENT_HAS_FIXED_PAYLOAD[index]) {
      printf("skipped event[%d] '%s': dynamic payload required\n", index, PROCESSOR_EVENT_NAMES[index]);
      continue;
    }
    if (!execution_succeeded(
      PROCESSOR_EVENT_FUNCTIONS[index](
        PROCESSOR_EVENT_DEFAULT_PAYLOADS[index],
        params,
        state,
        buffers,
        buffer_frames,
        buffer_channels,
        buffer_sample_rates
      ),
      "processor event"
    )) {
      goto cleanup;
    }
    printf("triggered event[%d] '%s' with its default payload\n", index, PROCESSOR_EVENT_NAMES[index]);
  }

  if (!execution_succeeded(
    onda_process(
      state,
      params,
      inputs,
      outputs,
      0,
      PROCESSOR_BLOCK_SIZE,
      ONDA_PROCESSOR_FULL_BLOCK,
      buffers,
      buffer_frames,
      buffer_channels,
      buffer_sample_rates
    ),
    "processor process"
  )) {
    goto cleanup;
  }

  printf("descriptor target: %s\n", PROCESSOR_TARGET_TRIPLE);
  for (int slot = 0; slot < PROCESSOR_OUTPUT_COUNT; ++slot) {
    printf(
      "output[%d] peak: %.6f\n",
      slot,
      peak(outputs[slot], PROCESSOR_OUTPUT_KINDS[slot])
    );
  }
  status = 0;

cleanup:
  if (buffers != NULL) {
    for (int index = 0; index < PROCESSOR_BUFFER_COUNT; ++index) {
      free(buffers[index]);
    }
  }
  if (outputs != NULL) {
    for (int slot = 0; slot < PROCESSOR_OUTPUT_COUNT; ++slot) {
      free(outputs[slot]);
    }
  }
  if (inputs != NULL) {
    for (int slot = 0; slot < PROCESSOR_INPUT_COUNT; ++slot) {
      free((void*)inputs[slot]);
    }
  }
  free(buffer_sample_rates);
  free(buffer_channels);
  free(buffer_frames);
  free(buffers);
  free(outputs);
  free(inputs);
  free(params);
  free(state);
  return status;
}
