// Query the resolved CPU, including features implied by its name. LLVM's C API
// only returns the feature overrides, which are insufficient for ISA selection.
#include <llvm/MC/MCSubtargetInfo.h>
#include <llvm/Target/TargetMachine.h>
#include <cstring>

extern "C" unsigned onda_vector_math_features(llvm::TargetMachine *machine) {
  const auto *features = machine->getMCSubtargetInfo();
  const auto has = [features](const char *name) {
    for (const auto &feature : features->getAllProcessorFeatures())
      if (std::strcmp(feature.Key, name) == 0)
        return features->hasFeature(feature.Value);
    return false;
  };
  const auto &triple = machine->getTargetTriple();
  if (triple.getArch() == llvm::Triple::x86_64) {
    return (has("sse2") ? 1 : 0) | (has("avx") ? 2 : 0) |
           (has("avx2") && has("fma") ? 4 : 0) |
           (has("avx512f") && has("avx2") && has("fma") ? 8 : 0);
  }
  if (triple.getArch() == llvm::Triple::aarch64)
    return has("neon") ? 16 : 0;
  return 0;
}
