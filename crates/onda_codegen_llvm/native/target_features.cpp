// Query the resolved CPU, including features implied by its name. LLVM's C API
// only returns the feature overrides, which are insufficient for ISA selection.
#include <llvm/MC/MCSubtargetInfo.h>
#include <llvm/Target/TargetMachine.h>
#include <cstring>

static bool has(llvm::TargetMachine *machine, const char *name) {
  const auto *features = machine->getMCSubtargetInfo();
  for (const auto &feature : features->getAllProcessorFeatures())
    if (std::strcmp(feature.Key, name) == 0)
      return features->hasFeature(feature.Value);
  return false;
}

extern "C" bool onda_expensive_short_masked_memory(llvm::TargetMachine *machine) {
  return machine->getTargetTriple().getArch() == llvm::Triple::x86_64 &&
         has(machine, "avx2") && !has(machine, "avx512f");
}

extern "C" unsigned onda_vector_math_features(llvm::TargetMachine *machine) {
  const auto &triple = machine->getTargetTriple();
  if (triple.getArch() == llvm::Triple::x86_64) {
    return (has(machine, "sse2") ? 1 : 0) | (has(machine, "avx") ? 2 : 0) |
           (has(machine, "avx2") && has(machine, "fma") ? 4 : 0) |
           (has(machine, "avx512f") && has(machine, "avx2") && has(machine, "fma") ? 8 : 0);
  }
  if (triple.getArch() == llvm::Triple::aarch64)
    return has(machine, "neon") ? 16 : 0;
  return 0;
}
