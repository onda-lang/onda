/* Optional Linux user-space counters. Read alongside timing, never inferred
 * from static instruction counts. Unavailable events are reported explicitly. */
#ifndef ONDA_BENCH_PERF_COUNTERS_H
#define ONDA_BENCH_PERF_COUNTERS_H
#include <errno.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#ifdef __linux__
#include <linux/perf_event.h>
#include <sys/ioctl.h>
#include <sys/syscall.h>
#include <unistd.h>
#endif

enum { PERF_EVENTS = 4 };
static const char *perf_names[PERF_EVENTS] = {
    "cycles", "instructions", "l1d_read_misses", "llc_read_misses"
};
typedef struct {
    int fd[PERF_EVENTS], error[PERF_EVENTS];
    uint64_t count[PERF_EVENTS], enabled[PERF_EVENTS], running[PERF_EVENTS];
} PerfCounters;

static void perf_open(PerfCounters *c) {
    memset(c, 0, sizeof(*c));
    for (int i = 0; i < PERF_EVENTS; ++i) {
        c->fd[i] = -1;
#ifdef __linux__
        struct perf_event_attr a = {0};
        a.size = sizeof(a); a.disabled = 1; a.exclude_kernel = 1; a.exclude_hv = 1;
        a.read_format = PERF_FORMAT_TOTAL_TIME_ENABLED | PERF_FORMAT_TOTAL_TIME_RUNNING;
        a.type = i < 2 ? PERF_TYPE_HARDWARE : PERF_TYPE_HW_CACHE;
        a.config = i == 0 ? PERF_COUNT_HW_CPU_CYCLES : i == 1 ? PERF_COUNT_HW_INSTRUCTIONS
            : (i == 2 ? PERF_COUNT_HW_CACHE_L1D : PERF_COUNT_HW_CACHE_LL)
                | ((uint64_t)PERF_COUNT_HW_CACHE_OP_READ << 8)
                | ((uint64_t)PERF_COUNT_HW_CACHE_RESULT_MISS << 16);
        c->fd[i] = syscall(SYS_perf_event_open, &a, 0, -1, -1, 0);
        if (c->fd[i] < 0) c->error[i] = errno;
#else
        c->error[i] = ENOSYS;
#endif
    }
}

static void perf_start(PerfCounters *c) {
#ifdef __linux__
    for (int i = 0; i < PERF_EVENTS; ++i) if (c->fd[i] >= 0) {
        if (ioctl(c->fd[i], PERF_EVENT_IOC_RESET, 0) < 0
            || ioctl(c->fd[i], PERF_EVENT_IOC_ENABLE, 0) < 0) c->error[i] = errno;
    }
#endif
}

static void perf_stop(PerfCounters *c) {
#ifdef __linux__
    for (int i = 0; i < PERF_EVENTS; ++i) if (c->fd[i] >= 0) {
        uint64_t values[3];
        if (ioctl(c->fd[i], PERF_EVENT_IOC_DISABLE, 0) < 0
            || read(c->fd[i], values, sizeof(values)) != sizeof(values)) {
            c->error[i] = errno ? errno : EIO;
        } else {
            c->count[i] = values[0]; c->enabled[i] = values[1]; c->running[i] = values[2];
        }
    }
#endif
}

static void perf_report(const PerfCounters *c, double calls) {
    printf("{");
    for (int i = 0; i < PERF_EVENTS; ++i) {
        printf("%s\"%s\":", i ? "," : "", perf_names[i]);
        if (c->fd[i] < 0 || c->error[i] || !c->running[i]) {
            printf("{\"available\":false,\"errno\":%d}", c->error[i]);
        } else {
            printf("{\"available\":true,\"per_call\":%.9f,\"running_fraction\":%.6f}",
                c->count[i] * ((double)c->enabled[i] / c->running[i]) / calls,
                (double)c->running[i] / c->enabled[i]);
        }
    }
    printf("}");
}

static void perf_close(PerfCounters *c) {
#ifdef __linux__
    for (int i = 0; i < PERF_EVENTS; ++i) if (c->fd[i] >= 0) close(c->fd[i]);
#endif
}
#endif
