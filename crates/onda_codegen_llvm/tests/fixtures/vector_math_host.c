/* Standalone object execution against the same MPFR intervals as the JIT tests.
   No Onda runtime, SLEEF library, or reference libm calls are used by this host. */
#include "onda_processor_abi.h"
#include <math.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>

#ifdef TEST_F64
typedef double sample;
typedef uint64_t raw_sample;
enum { precision = 64 };
#else
typedef float sample;
typedef uint32_t raw_sample;
enum { precision = 32 };
#endif

enum { operations = 10 };
static const char *const names[operations] = {
  "sin", "cos", "tan", "tanh", "atan", "atan2", "exp", "log", "pow", "exp2"
};

static sample from_bits(uint64_t bits) {
  raw_sample raw = (raw_sample)bits;
  sample result;
  memcpy(&result, &raw, sizeof(result));
  return result;
}

static raw_sample to_bits(sample value) {
  raw_sample result;
  memcpy(&result, &value, sizeof(result));
  return result;
}

static uint64_t ordered(sample value) {
  raw_sample raw = to_bits(value);
  raw_sample sign = (raw_sample)1 << (precision - 1);
  return raw & sign ? (raw_sample)~raw : raw | sign;
}

int main(int argc, char **argv) {
  if (argc != 2) return 1;
  FILE *references = fopen(argv[1], "r");
  if (!references) { perror(argv[1]); return 1; }
  sample left[128], right[128], output[operations][128];
  const void *inputs[] = {left, right};
  void *outputs[operations];
  const int starts[] = {0, 3, 0, 0};
  const int lengths[] = {1, 7, 127, 128};
  for (int op = 0; op < operations; ++op) outputs[op] = output[op];
  if (onda_processor_init(NULL, NULL, ONDA_PROCESSOR_INIT_FULL,
                          NULL, NULL, NULL, NULL, NULL) != 0) return 2;

  char line[256];
  unsigned tested[operations] = {0};
  while (fgets(line, sizeof(line), references)) {
    if (line[0] == '#') continue;
    char operation[16], kind;
    unsigned width;
    unsigned long long x, y, vector_lower, vector_upper, scalar_lower, scalar_upper;
    if (sscanf(line, "%15s %u %llx %llx %c %llx %llx %llx %llx",
               operation, &width, &x, &y, &kind, &vector_lower, &vector_upper,
               &scalar_lower, &scalar_upper) != 9) return 3;
    if (width != precision) continue;
    int selected = -1;
    for (int op = 0; op < operations; ++op)
      if (!strcmp(operation, names[op])) selected = op;
    if (selected < 0) return 3;
    for (int i = 0; i < 128; ++i) { left[i] = from_bits(x); right[i] = from_bits(y); }
    for (int round = 0; round < 4; ++round) {
      for (int op = 0; op < operations; ++op)
        for (int i = 0; i < 128; ++i) output[op][i] = -12345;
      int start = starts[round], frames = lengths[round];
      /* Full blocks exercise vector helpers. Short calls can mix both paths;
         apply the same numerical budget without SLEEF's rounded-zero rule. */
      unsigned long long lower = frames == 128 ? vector_lower : scalar_lower;
      unsigned long long upper = frames == 128 ? vector_upper : scalar_upper;
      if (onda_process(NULL, NULL, inputs, outputs, start, frames,
                       ONDA_PROCESSOR_FULL_BLOCK, NULL, NULL, NULL, NULL, NULL) != 0) return 4;
      for (int i = start; i < start + frames; ++i) {
        sample actual = output[selected][i];
        uint64_t key = ordered(actual);
        int ok = kind == 'N' ? isnan(actual) : key >= lower && key <= upper;
        if (!ok) {
          fprintf(stderr, "%s/f%u(%llx, %llx), frame %d, start %d, frames %d: "
                  "bits %llx, expected %c %llx..%llx\n", operation, width, x, y, i,
                  start, frames, (unsigned long long)to_bits(actual), kind, lower, upper);
          return 5;
        }
      }
      for (int op = 0; op < operations; ++op)
        for (int i = 0; i < 128; ++i)
          if ((i < start || i >= start + frames) &&
              to_bits(output[op][i]) != to_bits((sample)-12345)) return 6;
    }
    ++tested[selected];
  }
  int read_error = ferror(references);
  fclose(references);
  if (read_error) return 7;
  for (int op = 0; op < operations; ++op)
    if (!tested[op]) return 7;
  return 0;
}
