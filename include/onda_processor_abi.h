#ifndef ONDA_PROCESSOR_ABI_H
#define ONDA_PROCESSOR_ABI_H

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

/*
 * Decoded host-control metadata for one scalar numeric parameter.
 *
 * A descriptor without a numeric host-control domain uses scale NONE.
 * step_count == 0 means continuous. has_curve distinguishes an absent curve
 * from curve == 0. The descriptor contract guarantees finite values and the
 * additional scale/step invariants checked by onda_processor_param_domain_is_valid().
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
  uint8_t has_curve;
  const char* unit;
} onda_processor_param_domain;

#if defined(_MSC_VER)
#define ONDA_PROCESSOR_STATIC_INLINE static __inline
#else
#define ONDA_PROCESSOR_STATIC_INLINE static inline
#endif

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
    (domain->minimum <= 0.0 || domain->step_count != 0)
  ) {
    return 0;
  }
  return domain->step_count == 0 ||
    (isfinite(domain->step) && domain->step > 0.0);
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
    plain = domain->minimum
      + onda_processor_lincurve_normalized_to_unit(domain->curve, unit)
        * (domain->maximum - domain->minimum);
  } else if (domain->scale == ONDA_PROCESSOR_PARAM_SCALE_LOG) {
    const double log_minimum = log(domain->minimum);
    plain = exp(
      log_minimum + unit * (log(domain->maximum) - log_minimum)
    );
  } else {
    plain = domain->minimum + unit * (domain->maximum - domain->minimum);
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
  if (domain->has_curve) {
    normalized = onda_processor_lincurve_unit_to_normalized(
      domain->curve,
      (constrained - domain->minimum) / (domain->maximum - domain->minimum)
    );
  } else if (domain->scale == ONDA_PROCESSOR_PARAM_SCALE_LOG) {
    const double log_minimum = log(domain->minimum);
    normalized = (log(constrained) - log_minimum)
      / (log(domain->maximum) - log_minimum);
  } else {
    normalized = (constrained - domain->minimum)
      / (domain->maximum - domain->minimum);
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
