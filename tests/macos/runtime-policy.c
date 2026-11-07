#include <errno.h>
#include <stdio.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/types.h>
#include <unistd.h>

extern int csops(pid_t pid, unsigned int ops, void *address, size_t size);

int main(void) {
    unsigned int flags = 0;
    if (csops(getpid(), 0, &flags, sizeof(flags)) != 0) {
        perror("csops");
        return 1;
    }
    printf("Kernel signing flags: 0x%x; hardened runtime: %s\n", flags,
           (flags & 0x10000) ? "enabled" : "disabled");
    fflush(stdout);

#if defined(__aarch64__)
    const unsigned int code[] = {0xd2800540, 0xd65f03c0}; /* return 42 */
#else
    const unsigned char code[] = {0xb8, 42, 0, 0, 0, 0xc3};
#endif
    const size_t page_size = (size_t)getpagesize();
    void *memory = mmap(NULL, page_size, PROT_READ | PROT_WRITE,
                        MAP_PRIVATE | MAP_ANON, -1, 0);
    if (memory == MAP_FAILED) {
        perror("mmap");
        return 1;
    }
    memcpy(memory, code, sizeof(code));
    if (mprotect(memory, page_size, PROT_READ | PROT_EXEC) != 0) {
        perror("mprotect");
        return 1;
    }
    __builtin___clear_cache(memory, (char *)memory + sizeof(code));
    int result = ((int (*)(void))memory)();
    printf("JIT returned %d\n", result);
    munmap(memory, page_size);
    return result == 42 ? 0 : 1;
}
