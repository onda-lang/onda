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
  assert(ONDA_DELEGATE_RECORD_HEADER_SIZE == 8u);
  assert(ONDA_PROCESSOR_DELEGATE_RECORD_HEADER_SIZE == 8u);
  uint8_t storage[16] = {0};
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
  uint32_t header[2] = {3, 4};
  memcpy(storage, header, sizeof(header));
  memcpy(storage + sizeof(header), "test", 4);
  batch.used_bytes = 12;
  batch.record_count = 1;
  onda_processor_delegate_occurrence_t processor_occurrence;
  assert(onda_processor_delegate_batch_occurrence_at(&batch, 0, &processor_occurrence));
  assert(processor_occurrence.delegate_index == 3);
  assert(processor_occurrence.payload_size_bytes == 4);
  assert(memcmp(processor_occurrence.payload, "test", 4) == 0);
  assert(!onda_processor_delegate_batch_occurrence_at(&batch, 1, &processor_occurrence));

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
