/* Matched complete real FFT: external mono input -> split positive spectrum.
 * Both implementations consume N samples and expose N/2+1 real/imaginary bins.
 * Planning, allocation, initialization, validation and counters are untimed. */
#define _GNU_SOURCE
#include <fftw3.h>
#include <inttypes.h>
#include <math.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include "onda_processor_abi.h"
#include "perf_counters.h"
#if defined(__i386__) || defined(__x86_64__)
#include <xmmintrin.h>
#endif

#if FFT_SINGLE
typedef float Real;
typedef fftwf_plan Plan;
#define FFT(name) fftwf_##name
#else
typedef double Real;
typedef fftw_plan Plan;
#define FFT(name) fftw_##name
#endif
enum { BINS = FFT_SIZE / 2 + 1, PLANE_ELEMENTS = ((BINS * sizeof(Real) + 63) / 64) * 64 / sizeof(Real) };
typedef struct { void *state; Real *input, *real[2], *imag[2]; } Instance;
static Plan plan;
static int32_t frames[] = {FFT_SIZE}, channels[] = {1};
static float rates[] = {48000.0f};

static void *allocate(size_t bytes, size_t alignment) {
    void *result = NULL;
    if (posix_memalign(&result, alignment < sizeof(void *) ? sizeof(void *) : alignment, bytes)) exit(2);
    memset(result, 0, bytes);
    return result;
}
static double now(void) {
    struct timespec t; clock_gettime(CLOCK_MONOTONIC, &t);
    return t.tv_sec * 1e9 + t.tv_nsec;
}
static void execute(Instance *instance, int implementation) {
    if (implementation) {
        FFT(execute_split_dft_r2c)(plan, instance->input, instance->real[1], instance->imag[1]);
    } else {
        void *buffers[] = {instance->input};
        void *outputs[] = {instance->real[0], instance->imag[0]};
        if (onda_process(instance->state, NULL, NULL, outputs, 0, BINS,
                ONDA_PROCESSOR_FULL_BLOCK, buffers, frames, channels, rates, NULL)) exit(3);
    }
}

static void fill(Instance *instance, int pattern, uint32_t seed) {
    for (int i = 0; i < FFT_SIZE; ++i) {
        seed = seed * 1664525u + 1013904223u;
        double x = (double)(seed >> 8) / 8388608.0 - 1.0;
        if (pattern == 0) x = i == 0 ? 1.0 : 0.0;
        if (pattern == 1) x = 0.25;
        if (pattern == 2) x = sin(6.283185307179586 * 7 * i / FFT_SIZE);
        if (pattern == 3) x *= (i & 1) ? 1e-6 : 1e3;
        instance->input[i] = (Real)x;
    }
}

static uint64_t hash_bytes(uint64_t hash, const void *data, size_t length) {
    const unsigned char *bytes = data;
    for (size_t i = 0; i < length; ++i) hash = (hash ^ bytes[i]) * UINT64_C(1099511628211);
    return hash;
}

int main(int argc, char **argv) {
    if (argc != 4) return 1;
    int instances = atoi(argv[1]), rounds = atoi(argv[2]);
    double round_ns = strtod(argv[3], NULL) * 1e6;
    if (instances < 1 || instances > 4096 || rounds < 1 || rounds > 100 || !(round_ns > 0)) return 1;
#if defined(__i386__) || defined(__x86_64__)
    _mm_setcsr(_mm_getcsr() | 0x8040);
#endif
    Instance *data = calloc(instances, sizeof(*data));
    if (!data) return 2;
    for (int i = 0; i < instances; ++i) {
        data[i].state = allocate(ONDA_STATE_SIZE, ONDA_STATE_ALIGN);
        data[i].input = allocate(sizeof(Real) * FFT_SIZE, 64);
        for (int j = 0; j < 2; ++j) {
            // FFTW new-array split execution requires identical plane separation.
            data[i].real[j] = allocate(sizeof(Real) * PLANE_ELEMENTS * 2, 64);
            data[i].imag[j] = data[i].real[j] + PLANE_ELEMENTS;
        }
        void *buffers[] = {data[i].input};
        if (onda_processor_init(NULL, data[i].state, ONDA_PROCESSOR_INIT_FULL,
                buffers, frames, channels, rates, NULL)) return 3;
    }
    FFT(iodim) dimension = {FFT_SIZE, 1, 1};
    plan = FFT(plan_guru_split_dft_r2c)(1, &dimension, 0, NULL,
        data[0].input, data[0].real[1], data[0].imag[1], FFTW_MEASURE);
    if (!plan) return 4;
    double max_error = 0, max_normalized = 0;
    uint64_t output_hash = UINT64_C(14695981039346656037);
    for (int pattern = 0; pattern < 8; ++pattern) {
        fill(&data[0], pattern, 12345u + pattern);
        Real original[FFT_SIZE]; memcpy(original, data[0].input, sizeof(original));
        execute(&data[0], 0); execute(&data[0], 1);
        if (memcmp(original, data[0].input, sizeof(original))) return 5;
        double scale = 1;
        for (int i = 0; i < BINS; ++i) {
            scale = fmax(scale, fabs(data[0].real[1][i]));
            scale = fmax(scale, fabs(data[0].imag[1][i]));
        }
        for (int i = 0; i < BINS; ++i) for (int imaginary = 0; imaginary < 2; ++imaginary) {
            double a = imaginary ? data[0].imag[0][i] : data[0].real[0][i];
            double b = imaginary ? data[0].imag[1][i] : data[0].real[1][i];
            double error = fabs(a - b);
            if (!isfinite(a) || !isfinite(b) || error > scale * (FFT_SINGLE ? 2e-6 : 2e-13)) {
                fprintf(stderr, "FFT mismatch: pattern=%d bin=%d imaginary=%d Onda=%.17g FFTW=%.17g\n", pattern, i, imaginary, a, b);
                return 6;
            }
            max_error = fmax(max_error, error); max_normalized = fmax(max_normalized, error / scale);
        }
        output_hash = hash_bytes(output_hash, data[0].real[0], BINS * sizeof(Real));
        output_hash = hash_bytes(output_hash, data[0].imag[0], BINS * sizeof(Real));
    }
    for (int i = 0; i < instances; ++i) {
        fill(&data[i], 4, 12345u + i);
        execute(&data[i], 0); execute(&data[i], 1);
        double scale = 1;
        for (int k = 0; k < BINS; ++k) {
            scale = fmax(scale, fabs(data[i].real[1][k]));
            scale = fmax(scale, fabs(data[i].imag[1][k]));
        }
        for (int k = 0; k < BINS; ++k) {
            double re = fabs(data[i].real[0][k] - data[i].real[1][k]);
            double im = fabs(data[i].imag[0][k] - data[i].imag[1][k]);
            if (!isfinite(re) || !isfinite(im) || fmax(re, im) > scale * (FFT_SINGLE ? 2e-6 : 2e-13)) return 6;
        }
        output_hash = hash_bytes(output_hash, data[i].real[0], BINS * sizeof(Real));
        output_hash = hash_bytes(output_hash, data[i].imag[0], BINS * sizeof(Real));
    }
    for (int i = 0; i < 512; ++i) for (int j = 0; j < 2; ++j) execute(&data[i % instances], j);
    int iterations = 256;
    for (;;) {
        double start = now();
        for (int i = 0; i < iterations; ++i) execute(&data[i % instances], 0);
        if (now() - start >= round_ns) break;
        if (iterations >= (1 << 26)) return 7;
        iterations *= 2;
    }
    printf("{\"size\":%d,\"precision\":\"%s\",\"instances\":%d,\"iterations\":%d,"
           "\"validated_bins\":%d,\"max_absolute_error\":%.17g,\"max_normalized_error\":%.17g,"
           "\"onda_output_hash\":\"%016" PRIx64 "\","
           "\"fftw_version\":\"%s\",\"rounds\":[",
        FFT_SIZE, FFT_SINGLE ? "f32" : "f64", instances, iterations, BINS * 8,
        max_error, max_normalized, output_hash, FFT(version));
    for (int round = 0; round < rounds; ++round) for (int order = 0; order < 2; ++order) {
        int implementation = (round + order) % 2;
        double start = now();
        for (int i = 0; i < iterations; ++i) execute(&data[i % instances], implementation);
        double elapsed = (now() - start) / iterations;
        printf("%s{\"implementation\":\"%s\",\"ns_per_transform\":%.9f}",
            round || order ? "," : "", implementation ? "fftw" : "onda", elapsed);
    }
    printf("],\"counters\":{");
    for (int implementation = 0; implementation < 2; ++implementation) {
        PerfCounters counters; perf_open(&counters); perf_start(&counters);
        for (int i = 0; i < iterations; ++i) execute(&data[i % instances], implementation);
        perf_stop(&counters);
        printf("%s\"%s\":", implementation ? "," : "", implementation ? "fftw" : "onda");
        perf_report(&counters, iterations); perf_close(&counters);
    }
    printf("}}\n");
    FFT(destroy_plan)(plan);
    for (int i = 0; i < instances; ++i) {
        free(data[i].state); free(data[i].input);
        for (int j = 0; j < 2; ++j) free(data[i].real[j]);
    }
    free(data);
    return 0;
}
