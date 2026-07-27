#ifndef ONDA_PROCESSOR_ABI_H
#define ONDA_PROCESSOR_ABI_H

#include <float.h>
#include <math.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Synchronized from format-versions.json; do not edit this copy directly. */
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

typedef enum onda_processor_param_scale {
  ONDA_PROCESSOR_PARAM_SCALE_NONE = 0,
  ONDA_PROCESSOR_PARAM_SCALE_LINEAR = 1,
  ONDA_PROCESSOR_PARAM_SCALE_LOG = 2
} onda_processor_param_scale;

typedef enum onda_processor_param_scalar {
  ONDA_PROCESSOR_PARAM_SCALAR_F32 = 0,
  ONDA_PROCESSOR_PARAM_SCALAR_F64 = 1,
  ONDA_PROCESSOR_PARAM_SCALAR_I32 = 2,
  ONDA_PROCESSOR_PARAM_SCALAR_I64 = 3
} onda_processor_param_scalar;

/*
 * Decoded host-control metadata for one scalar numeric parameter.
 *
 * A descriptor without a numeric host-control domain uses scale NONE.
 * step_count == 0 means continuous. has_curve distinguishes an absent curve
 * from curve == 0. scalar is the parameter's declared storage scalar. The
 * descriptor contract guarantees finite values and the additional scale/step
 * invariants checked by onda_processor_param_domain_is_valid().
 * unit is optional display text; the caller retains ownership of the pointed-to
 * NUL-terminated string for as long as the domain is used.
 */
typedef struct onda_processor_param_domain {
  double minimum;
  double maximum;
  double step;
  double curve;
  uint32_t step_count;
  onda_processor_param_scale scale;
  onda_processor_param_scalar scalar;
  uint8_t has_curve;
  const char* unit;
} onda_processor_param_domain;

#if defined(_MSC_VER)
#define ONDA_PROCESSOR_STATIC_INLINE static __inline
#else
#define ONDA_PROCESSOR_STATIC_INLINE static inline
#endif

ONDA_PROCESSOR_STATIC_INLINE int onda_processor_float_grid_value_matches(
  onda_processor_param_scalar scalar,
  double minimum,
  double expected,
  double step,
  uint32_t index
) {
  const double scaled_step = step * (double)index;
  const double reconstructed = minimum + scaled_step;
  if (!isfinite(reconstructed)) {
    return 0;
  }
  if (scalar == ONDA_PROCESSOR_PARAM_SCALAR_F32) {
    return (float)reconstructed == (float)expected;
  }
  if (scalar != ONDA_PROCESSOR_PARAM_SCALAR_F64) {
    return 0;
  }
  const double scale = fmax(
    fmax(fabs(minimum), fabs(expected)),
    fmax(fabs(scaled_step), DBL_MIN)
  );
  const double rounding_tolerance = 8.0 * DBL_EPSILON * scale;
  const double grid_tolerance = 0.125 * step;
  return fabs(reconstructed - expected) <=
    fmin(rounding_tolerance, grid_tolerance);
}

ONDA_PROCESSOR_STATIC_INLINE int onda_processor_integer_domain_value_is_valid(
  onda_processor_param_scalar scalar,
  double value
) {
  if (!isfinite(value) || trunc(value) != value) {
    return 0;
  }
  if (scalar == ONDA_PROCESSOR_PARAM_SCALAR_I32) {
    return value >= (double)INT32_MIN && value <= (double)INT32_MAX;
  }
  if (scalar == ONDA_PROCESSOR_PARAM_SCALAR_I64) {
    return fabs(value) <= 9007199254740991.0;
  }
  return 0;
}

ONDA_PROCESSOR_STATIC_INLINE int onda_processor_param_domain_is_valid(
  const onda_processor_param_domain* domain
) {
  if (
    domain == NULL ||
    domain->scale == ONDA_PROCESSOR_PARAM_SCALE_NONE ||
    !isfinite(domain->minimum) ||
    !isfinite(domain->maximum) ||
    domain->minimum >= domain->maximum
  ) {
    return 0;
  }
  if (
    domain->scale != ONDA_PROCESSOR_PARAM_SCALE_LINEAR &&
    domain->scale != ONDA_PROCESSOR_PARAM_SCALE_LOG
  ) {
    return 0;
  }
  if (
    domain->scalar != ONDA_PROCESSOR_PARAM_SCALAR_F32 &&
    domain->scalar != ONDA_PROCESSOR_PARAM_SCALAR_F64 &&
    domain->scalar != ONDA_PROCESSOR_PARAM_SCALAR_I32 &&
    domain->scalar != ONDA_PROCESSOR_PARAM_SCALAR_I64
  ) {
    return 0;
  }
  if (
    domain->scalar == ONDA_PROCESSOR_PARAM_SCALAR_F32 &&
    (
      (double)(float)domain->minimum != domain->minimum ||
      (double)(float)domain->maximum != domain->maximum ||
      (
        domain->step_count != 0 &&
        (double)(float)domain->step != domain->step
      )
    )
  ) {
    return 0;
  }
  if (
    (
      domain->scalar == ONDA_PROCESSOR_PARAM_SCALAR_I32 ||
      domain->scalar == ONDA_PROCESSOR_PARAM_SCALAR_I64
    ) &&
    (
      !onda_processor_integer_domain_value_is_valid(
        domain->scalar,
        domain->minimum
      ) ||
      !onda_processor_integer_domain_value_is_valid(
        domain->scalar,
        domain->maximum
      )
    )
  ) {
    return 0;
  }
  if (
    domain->scalar == ONDA_PROCESSOR_PARAM_SCALAR_I64 &&
    domain->maximum - domain->minimum > 9007199254740991.0
  ) {
    return 0;
  }
  if (domain->has_curve) {
    if (
      domain->scale != ONDA_PROCESSOR_PARAM_SCALE_LINEAR ||
      !isfinite(domain->curve)
    ) {
      return 0;
    }
  }
  if (
    domain->scale == ONDA_PROCESSOR_PARAM_SCALE_LOG &&
    (
      domain->minimum <= 0.0 ||
      domain->step_count != 0 ||
      (
        domain->scalar != ONDA_PROCESSOR_PARAM_SCALAR_F32 &&
        domain->scalar != ONDA_PROCESSOR_PARAM_SCALAR_F64
      )
    )
  ) {
    return 0;
  }
  if (domain->step_count == 0) {
    return domain->scalar == ONDA_PROCESSOR_PARAM_SCALAR_F32 ||
      domain->scalar == ONDA_PROCESSOR_PARAM_SCALAR_F64;
  }
  if (!isfinite(domain->step) || domain->step <= 0.0) {
    return 0;
  }
  if (
    domain->scalar == ONDA_PROCESSOR_PARAM_SCALAR_F32 ||
    domain->scalar == ONDA_PROCESSOR_PARAM_SCALAR_F64
  ) {
    return onda_processor_float_grid_value_matches(
      domain->scalar,
      domain->minimum,
      domain->maximum,
      domain->step,
      domain->step_count
    );
  }
  if (
    domain->scalar == ONDA_PROCESSOR_PARAM_SCALAR_I32 ||
    domain->scalar == ONDA_PROCESSOR_PARAM_SCALAR_I64
  ) {
    if (
      !onda_processor_integer_domain_value_is_valid(
        domain->scalar,
        domain->step
      )
    ) {
      return 0;
    }
    const int64_t minimum = (int64_t)domain->minimum;
    const int64_t maximum = (int64_t)domain->maximum;
    const int64_t step = (int64_t)domain->step;
    const int64_t width = maximum - minimum;
    return width % step == 0 &&
      width / step == (int64_t)domain->step_count;
  }
  return 0;
}

ONDA_PROCESSOR_STATIC_INLINE double onda_processor_lincurve_normalized_to_unit(
  double curve,
  double normalized
) {
  if (fabs(curve) < 0.001) {
    return normalized;
  }
  if (curve > 0.0) {
    const double reflected = 1.0 - normalized;
    return 1.0 - expm1(-curve * reflected) / expm1(-curve);
  }
  return expm1(curve * normalized) / expm1(curve);
}

ONDA_PROCESSOR_STATIC_INLINE double onda_processor_lincurve_unit_to_normalized(
  double curve,
  double unit
) {
  if (fabs(curve) < 0.001) {
    return unit;
  }
  if (curve > 0.0) {
    const double reflected = 1.0 - unit;
    return 1.0 - log1p(reflected * expm1(-curve)) / -curve;
  }
  return log1p(unit * expm1(curve)) / curve;
}

ONDA_PROCESSOR_STATIC_INLINE double onda_processor_linear_unit_to_plain(
  double minimum,
  double maximum,
  double unit
) {
  const double width = maximum - minimum;
  return isfinite(width)
    ? minimum + unit * width
    : (1.0 - unit) * minimum + unit * maximum;
}

ONDA_PROCESSOR_STATIC_INLINE double onda_processor_linear_plain_to_unit(
  double minimum,
  double maximum,
  double plain
) {
  const double width = maximum - minimum;
  if (isfinite(width)) {
    return (plain - minimum) / width;
  }
  const double scale = fmax(fabs(minimum), fabs(maximum));
  return (
    (plain / scale) - (minimum / scale)
  ) / (
    (maximum / scale) - (minimum / scale)
  );
}

ONDA_PROCESSOR_STATIC_INLINE double onda_processor_param_constrain_plain(
  const onda_processor_param_domain* domain,
  double plain
) {
  if (!onda_processor_param_domain_is_valid(domain)) {
    return NAN;
  }
  double constrained = isnan(plain)
    ? domain->minimum
    : fmin(domain->maximum, fmax(domain->minimum, plain));
  if (domain->step_count != 0) {
    constrained = domain->minimum
      + floor((constrained - domain->minimum) / domain->step + 0.5)
        * domain->step;
    constrained = fmin(domain->maximum, fmax(domain->minimum, constrained));
  }
  return constrained;
}

ONDA_PROCESSOR_STATIC_INLINE double onda_processor_param_normalized_to_plain(
  const onda_processor_param_domain* domain,
  double normalized
) {
  if (!onda_processor_param_domain_is_valid(domain)) {
    return NAN;
  }
  const double unit = isnan(normalized)
    ? 0.0
    : fmin(1.0, fmax(0.0, normalized));
  if (unit == 0.0) {
    return domain->minimum;
  }
  if (unit == 1.0) {
    return domain->maximum;
  }

  double plain;
  if (domain->has_curve) {
    const double curved =
      onda_processor_lincurve_normalized_to_unit(domain->curve, unit);
    plain = onda_processor_linear_unit_to_plain(
      domain->minimum,
      domain->maximum,
      curved
    );
  } else if (domain->scale == ONDA_PROCESSOR_PARAM_SCALE_LOG) {
    const double log_minimum = log(domain->minimum);
    plain = exp(
      log_minimum + unit * (log(domain->maximum) - log_minimum)
    );
  } else {
    plain = onda_processor_linear_unit_to_plain(
      domain->minimum,
      domain->maximum,
      unit
    );
  }
  return onda_processor_param_constrain_plain(domain, plain);
}

ONDA_PROCESSOR_STATIC_INLINE double onda_processor_param_plain_to_normalized(
  const onda_processor_param_domain* domain,
  double plain
) {
  const double constrained = onda_processor_param_constrain_plain(domain, plain);
  if (isnan(constrained)) {
    return NAN;
  }
  if (constrained == domain->minimum) {
    return 0.0;
  }
  if (constrained == domain->maximum) {
    return 1.0;
  }

  double normalized;
  const double linear_unit = onda_processor_linear_plain_to_unit(
    domain->minimum,
    domain->maximum,
    constrained
  );
  if (domain->has_curve) {
    normalized = onda_processor_lincurve_unit_to_normalized(
      domain->curve,
      linear_unit
    );
  } else if (domain->scale == ONDA_PROCESSOR_PARAM_SCALE_LOG) {
    const double log_minimum = log(domain->minimum);
    normalized = (log(constrained) - log_minimum)
      / (log(domain->maximum) - log_minimum);
  } else {
    normalized = linear_unit;
  }
  return fmin(1.0, fmax(0.0, normalized));
}

#undef ONDA_PROCESSOR_STATIC_INLINE

/*
 * ABI symbols emitted by every native processor object. Pointer tables and
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
