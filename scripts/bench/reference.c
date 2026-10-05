/* Independent scalar C kernels and SIMD-enabled FFTW real transforms.
 * cc -O3 -march=native -ffp-contract=off reference.c -lfftw3f -lfftw3 -lm -o reference
 * reference NAME BLOCK_SIZE OUTPUT_PREFIX SAMPLE_RATE REPETITIONS ROUND_MS WARMUP VALIDATION_BLOCKS
 */
#define _POSIX_C_SOURCE 200809L
#include <fftw3.h>
#include <math.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#if defined(__i386__) || defined(__x86_64__)
#include <xmmintrin.h>
#endif

static double now_ns(void) {
    struct timespec t;
    clock_gettime(CLOCK_MONOTONIC, &t);
    return t.tv_sec * 1e9 + t.tv_nsec;
}
static int compare(const void *a, const void *b) {
    double x = *(const double *)a, y = *(const double *)b;
    return (x > y) - (x < y);
}
static void report(double *samples, int count, double divisor, double check) {
    double *deviations = calloc(count, sizeof(double));
    if (!deviations) exit(1);
    printf("{\"rounds_ns_per_frame\":[");
    for (int i = 0; i < count; ++i) printf("%s%.9f", i ? "," : "", samples[i] / divisor);
    printf("],");
    qsort(samples, count, sizeof(double), compare);
    double median = samples[count / 2];
    for (int i = 0; i < count; ++i) deviations[i] = fabs(samples[i] - median);
    qsort(deviations, count, sizeof(double), compare);
    printf("\"ns_per_frame\":%.9f,\"mad_ns\":%.9f,\"check\":%.9g,"
           "\"fftw_f32_version\":\"%s\",\"fftw_f64_version\":\"%s\"}\n",
           median / divisor, deviations[count / 2] / divisor, check, fftwf_version, fftw_version);
    free(deviations);
    free(samples);
}

/* Keep the execution boundary like the generated Onda block entrypoint. */
typedef struct { float z1, z2, x1, a, b0, b1, b2, a1, a2, f, iq, d, g1; } State;
__attribute__((noinline)) static void scalar(const char *name, State *restrict s,
                                            float *restrict input, float *restrict output, int n) {
    if (!strcmp(name, "passthrough")) { memcpy(output, input, n * sizeof(float)); return; }
    if (!strcmp(name, "filter-onepole")) {
        float z = s->z1;
        for (int i = 0; i < n; ++i) { z += s->a * (input[i] - z); output[i] = z; }
        s->z1 = z;
    } else if (!strcmp(name, "filter-dcblock")) {
        float x1 = s->x1, z = s->z1;
        for (int i = 0; i < n; ++i) {
            float x = input[i], y = x - x1 + 0.995f * z;
            x1 = x; z = y; output[i] = y;
        }
        s->x1 = x1; s->z1 = z;
    } else if (!strcmp(name, "filter-biquad")) {
        float z1 = s->z1, z2 = s->z2;
        for (int i = 0; i < n; ++i) {
            float x = input[i], y = s->b0 * x + z1;
            z1 = s->b1 * x - s->a1 * y + z2;
            z2 = s->b2 * x - s->a2 * y; output[i] = y;
        }
        s->z1 = z1; s->z2 = z2;
    } else if (!strcmp(name, "filter-svf")) {
        float z1 = s->z1, z2 = s->z2;
        for (int i = 0; i < n; ++i) {
            float high = (input[i] - s->g1 * z1 - z2) * s->d;
            float band = s->f * high + z1, low = s->f * band + z2;
            z1 = s->f * high + band; z2 = s->f * band + low; output[i] = low;
        }
        s->z1 = z1; s->z2 = z2;
    } else { fprintf(stderr, "unknown scalar case\n"); exit(1); }

}

int main(int argc, char **argv) {
    if (argc != 9) return 1;
    const char *name = argv[1];
    int bs = atoi(argv[2]);
    if (bs <= 0) return 1;
    float sample_rate = strtof(argv[4], NULL);
    int repetitions = atoi(argv[5]), warmup = atoi(argv[7]), validation_blocks = atoi(argv[8]);
    double round_ns = strtod(argv[6], NULL) * 1e6;
    if (!isfinite(sample_rate) || sample_rate <= 0 || repetitions <= 0 || warmup < 0
        || validation_blocks <= 0 || !isfinite(round_ns) || round_ns <= 0) return 1;
#if defined(__i386__) || defined(__x86_64__)
    _mm_setcsr(_mm_getcsr() | 0x8040); /* Onda native FTZ/DAZ policy. */
#endif
    double *samples = calloc(repetitions, sizeof(double));
    if (!samples) return 1;
    if (!strncmp(name, "fft-real-", 9)) {
        int n; char width[4];
        if (sscanf(name, "fft-real-%3[^-]-%d", width, &n) != 2 || n < 16) return 1;
        int single = !strcmp(width, "f32");
        void *input = single ? (void *)fftwf_alloc_real(n) : (void *)fftw_alloc_real(n);
        void *output = single ? (void *)fftwf_alloc_complex(n / 2 + 1) : (void *)fftw_alloc_complex(n / 2 + 1);
        if (!input || !output) return 1;
        fftwf_plan fp = NULL; fftw_plan dp = NULL;
        if (single) fp = fftwf_plan_dft_r2c_1d(n, input, output, FFTW_MEASURE);
        else dp = fftw_plan_dft_r2c_1d(n, input, output, FFTW_MEASURE);
        if (single ? !fp : !dp) return 1;
        for (int i = 0; i < n; ++i) {
            float value = sinf((float)i * 0.017f) + 0.25f * cosf((float)i * 0.061f);
            if (single) ((float *)input)[i] = value; else ((double *)input)[i] = value;
        }
        for (int i = 0; i < warmup; ++i) { if (single) fftwf_execute(fp); else fftw_execute(dp); }
        int iterations = 256;
        for (;;) {
            double start = now_ns();
            for (int i = 0; i < iterations; ++i) { if (single) fftwf_execute(fp); else fftw_execute(dp); }
            if (now_ns() - start >= round_ns) break;
            if (iterations > (1 << 27)) return 1;
            iterations *= 2;
        }
        for (int r = 0; r < repetitions; ++r) {
            double start = now_ns();
            for (int i = 0; i < iterations; ++i) { if (single) fftwf_execute(fp); else fftw_execute(dp); }
            samples[r] = (now_ns() - start) / iterations;
        }
        double check = single ? ((fftwf_complex *)output)[7][0] : ((fftw_complex *)output)[7][0];
        report(samples, repetitions, bs, check);
        if (single) { fftwf_destroy_plan(fp); fftwf_free(input); fftwf_free(output); }
        else { fftw_destroy_plan(dp); fftw_free(input); fftw_free(output); }
        return 0;
    }
    float *input = calloc(bs, sizeof(float)), *output = calloc(bs, sizeof(float));
    if (!input || !output) return 1;
    uint32_t seed = 12345;
    for (int i = 0; i < bs; ++i) {
        seed = seed * 1664525u + 1013904223u;
        input[i] = (float)(seed >> 8) * (1.f / 16777216.f) * 1.6f - 0.8f;
    }
    float omega = 6.283185307179586f * 1000.f / sample_rate;
    float cosine = cosf(omega), alpha = sinf(omega) / (2.f * 0.707107f);
    float inv = 1.f / (1.f + alpha);
    State s = {0};
    s.a = 1.f - expf(-6.283185307179586f * 1000.f / sample_rate);
    s.b0 = (1.f - cosine) * 0.5f * inv; s.b1 = (1.f - cosine) * inv; s.b2 = s.b0;
    s.a1 = -2.f * cosine * inv; s.a2 = (1.f - alpha) * inv;
    s.f = tanf(3.141592653589793f * 1000.f / sample_rate); s.iq = 1.f / 0.707107f;
    s.d = 1.f / (1.f + s.iq * s.f + s.f * s.f); s.g1 = s.f + s.iq;
    char path[4096];
    if (snprintf(path, sizeof(path), "%s.output.bin", argv[3]) >= (int)sizeof(path)) return 1;
    FILE *file = fopen(path, "wb");
    if (!file) return 1;
    for (int i = 0; i < validation_blocks; ++i) {
        scalar(name, &s, input, output, bs);
        if (fwrite(output, sizeof(float), bs, file) != (size_t)bs) return 1;
    }
    fclose(file);
    for (int i = 0; i < warmup; ++i) scalar(name, &s, input, output, bs);
    if (snprintf(path, sizeof(path), "%s.warm-output.bin", argv[3]) >= (int)sizeof(path)) return 1;
    file = fopen(path, "wb");
    if (!file) return 1;
    for (int i = 0; i < validation_blocks; ++i) {
        scalar(name, &s, input, output, bs);
        if (fwrite(output, sizeof(float), bs, file) != (size_t)bs) return 1;
    }
    fclose(file);
    int iterations = 256;
    for (;;) {
        double start = now_ns();
        for (int i = 0; i < iterations; ++i) scalar(name, &s, input, output, bs);
        if (now_ns() - start >= round_ns) break;
        if (iterations > (1 << 27)) return 1;
        iterations *= 2;
    }
    for (int r = 0; r < repetitions; ++r) {
        double start = now_ns();
        for (int i = 0; i < iterations; ++i) scalar(name, &s, input, output, bs);
        samples[r] = (now_ns() - start) / iterations;
    }
    report(samples, repetitions, bs, output[bs - 1]);
    free(input); free(output);
    return 0;
}
