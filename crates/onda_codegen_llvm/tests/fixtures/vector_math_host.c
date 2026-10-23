#include "onda_processor_abi.h"
#include <float.h>
#include <math.h>
#include <stdio.h>

#ifdef TEST_F64
typedef double sample;
#define REF(name) name
#define EPSILON DBL_EPSILON
#else
typedef float sample;
#define REF(name) name##f
#define EPSILON FLT_EPSILON
#endif

enum { operations = 10 };

int main(void) {
  sample input[128], output[operations][128];
  const void *inputs[] = {input};
  void *outputs[operations];
  const int lengths[] = {1, 7, 31, 127, 128};
  for (int i = 0; i < 128; ++i) input[i] = (sample)(0.025 + i * 0.005);
  input[0] = NAN;
  input[1] = INFINITY;
  input[2] = -INFINITY;
  input[3] = 0;
  input[4] = -0.0;
  input[5] = (sample)1e20;
  input[6] = (sample)1e-20;
  for (int op = 0; op < operations; ++op) outputs[op] = output[op];
  if (onda_processor_init(NULL, NULL, ONDA_PROCESSOR_INIT_FULL,
                          NULL, NULL, NULL, NULL, NULL) != 0) return 1;
  for (int round = 0; round < 5; ++round) {
    for (int op = 0; op < operations; ++op)
      for (int i = 0; i < 128; ++i) output[op][i] = -9999;
    int frames = lengths[round];
    if (onda_process(NULL, NULL, inputs, outputs, 0, frames,
                     ONDA_PROCESSOR_FULL_BLOCK, NULL, NULL, NULL, NULL, NULL) != 0)
      return 2;
    for (int i = 0; i < frames; ++i) {
      sample x = input[i];
      sample expected[] = {REF(sin)(x), REF(cos)(x), REF(tan)(x), REF(tanh)(x),
        REF(atan)(x), REF(atan2)(x, (sample)0.5), REF(exp)(x), REF(log)(x), REF(pow)(x, x), REF(exp2)(x)};
      for (int op = 0; op < operations; ++op) {
        sample actual = output[op][i], reference = expected[op];
        int ok = isnan(reference) ? isnan(actual) :
          !isfinite(reference) || reference == 0 ?
            actual == reference && !!signbit(actual) == !!signbit(reference) :
            fabs(actual - reference) <= 5 * EPSILON * fabs(reference);
        if (!ok) {
          fprintf(stderr, "op %d, frame %d/%d: %.17g vs %.17g\n",
                  op, i, frames, (double)actual, (double)reference);
          return 3;
        }
      }
    }
    for (int op = 0; op < operations; ++op)
      for (int i = frames; i < 128; ++i)
        if (output[op][i] != -9999) return 4;
  }
  return 0;
}
