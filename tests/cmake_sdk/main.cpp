#include <onda.h>

#if defined(_WIN32)
#define ONDA_SMOKE_EXPORT __declspec(dllexport)
#else
#define ONDA_SMOKE_EXPORT __attribute__((visibility("default")))
#endif

extern "C" ONDA_SMOKE_EXPORT int onda_cmake_sdk_smoke() {
  onda_compile_options_t options{};
  options.sample_rate = 48'000.0F;
  options.block_size = 64;

  onda_diag_t diagnostic{};
  auto *program = onda_compile("outs { out1 }\nsample { out1 = 0.0 }\n",
                               &options, &diagnostic);
  if (program == nullptr)
    return 1;
  onda_program_destroy(program);
  return 0;
}
