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
