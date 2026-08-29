#include "../include/onda.h"
#include "../include/onda_processor_abi.h"

#include <assert.h>

static onda_processor_param_domain integer_domain(
  onda_processor_param_scalar scalar,
  double minimum,
  double maximum,
  double step,
  uint32_t step_count
) {
  onda_processor_param_domain domain = {0};
  domain.minimum = minimum;
  domain.maximum = maximum;
  domain.step = step;
  domain.step_count = step_count;
  domain.scale = ONDA_PROCESSOR_PARAM_SCALE_LINEAR;
  domain.scalar = scalar;
  return domain;
}

int main(void) {
  assert(ONDA_DELEGATE_RECORD_HEADER_SIZE == 12u);
  assert(ONDA_PRINT_RECORD_HEADER_SIZE == 12u);
  assert(ONDA_PROCESSOR_DELEGATE_RECORD_HEADER_SIZE == 12u);
  assert(ONDA_PROCESSOR_PRINT_RECORD_HEADER_SIZE == 12u);
  uint8_t storage[40] = {0};
  onda_delegate_batch_t hosted_batch = {
    storage,
    sizeof(storage),
    0,
    0,
    0,
  };
  onda_delegate_occurrence_t occurrence = {0};
  assert(hosted_batch.storage == storage);
  assert(occurrence.payload == NULL);

  onda_processor_delegate_batch_t batch = {
    storage,
    sizeof(storage),
    12,
    1,
    2,
  };
  onda_processor_delegate_batch_reset(&batch);
  assert(batch.used_bytes == 0);
  assert(batch.record_count == 0);
  assert(batch.overflow_count == 0);
  uint32_t header[3] = {3, 4, 7};
  memcpy(storage, header, sizeof(header));
  memcpy(storage + sizeof(header), "test", 4);
  uint32_t second_header[3] = {4, 4, 8};
  memcpy(storage + 16, second_header, sizeof(second_header));
  memcpy(storage + 28, "next", 4);
  batch.used_bytes = 32;
  batch.record_count = 2;
  onda_processor_delegate_occurrence_t processor_occurrence;
  onda_processor_batch_cursor_t cursor = {0};
  assert(onda_processor_delegate_batch_next(&batch, &cursor, &processor_occurrence));
  assert(processor_occurrence.delegate_index == 3);
  assert(processor_occurrence.payload_size_bytes == 4);
  assert(processor_occurrence.sequence == 7);
  assert(memcmp(processor_occurrence.payload, "test", 4) == 0);
  assert(onda_processor_delegate_batch_next(&batch, &cursor, &processor_occurrence));
  assert(processor_occurrence.delegate_index == 4);
  assert(processor_occurrence.sequence == 8);
  assert(memcmp(processor_occurrence.payload, "next", 4) == 0);
  assert(!onda_processor_delegate_batch_next(&batch, &cursor, &processor_occurrence));
  assert(onda_processor_delegate_batch_occurrence_at(&batch, 1, &processor_occurrence));

  onda_processor_print_batch_t print_batch = {
    storage,
    sizeof(storage),
    0,
    0,
    0,
  };
  onda_processor_execution_output_t execution_output = {&batch, &print_batch, 9};
  assert(execution_output.delegate_batch == &batch);
  assert(execution_output.print_batch == &print_batch);
  onda_processor_execution_output_reset(&execution_output);
  assert(batch.used_bytes == 0);
  assert(batch.record_count == 0);
  assert(batch.overflow_count == 0);
  assert(print_batch.used_bytes == 0);
  assert(print_batch.record_count == 0);
  assert(print_batch.overflow_count == 0);
  assert(execution_output.next_sequence == 0);
  onda_processor_print_batch_reset(&print_batch);
  uint32_t print_header[3] = {5, 1, 9};
  memcpy(storage, print_header, sizeof(print_header));
  storage[12] = 1;
  print_batch.used_bytes = 13;
  print_batch.record_count = 1;
  onda_processor_print_occurrence_t print_occurrence;
  onda_processor_batch_cursor_t print_cursor = {0};
  assert(onda_processor_print_batch_next(&print_batch, &print_cursor, &print_occurrence));
  assert(print_occurrence.site_index == 5);
  assert(print_occurrence.payload_size_bytes == 1);
  assert(print_occurrence.sequence == 9);
  assert(print_occurrence.payload[0] == 1);

  onda_processor_param_domain domain =
    integer_domain(ONDA_PROCESSOR_PARAM_SCALAR_I32, 0.0, 10.0, 1.0, 10);
  assert(onda_processor_param_domain_is_valid(&domain));

  domain.minimum = 0.5;
  assert(!onda_processor_param_domain_is_valid(&domain));
  domain = integer_domain(
    ONDA_PROCESSOR_PARAM_SCALAR_I32,
    0.0,
    2147483648.0,
    1.0,
    1
  );
  assert(!onda_processor_param_domain_is_valid(&domain));
  domain = integer_domain(
    ONDA_PROCESSOR_PARAM_SCALAR_I32,
    0.0,
    10.0,
    0.5,
    20
  );
  assert(!onda_processor_param_domain_is_valid(&domain));

  domain = integer_domain(
    ONDA_PROCESSOR_PARAM_SCALAR_I64,
    0.0,
    9007199254740991.0,
    9007199254740991.0,
    1
  );
  assert(onda_processor_param_domain_is_valid(&domain));

  domain = integer_domain(
    ONDA_PROCESSOR_PARAM_SCALAR_I64,
    -9007199254740991.0,
    9007199254740991.0,
    9007199254740991.0,
    2
  );
  assert(!onda_processor_param_domain_is_valid(&domain));

  domain.maximum = 9007199254740992.0;
  assert(!onda_processor_param_domain_is_valid(&domain));

  return 0;
}
