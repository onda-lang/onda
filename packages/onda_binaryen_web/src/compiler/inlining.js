import binaryen from "binaryen";

// Binaryen's inline annotation encodes the inlining preference in one byte.
const NEVER = "00";
const ALWAYS = "7f";

// Binaryen 132 accepts inline annotations on declarations in WAT. Its JS API
// can fill in their signatures, locals and bodies, but cannot set the hint.
export function createModuleWithInlineHints(functions, names, hintOnlyPass, kernel, roundHelpers) {
  const never = [];
  const heuristic = [];
  const always = [];
  for (const [id, func] of functions.entries()) {
    switch (func.attributes.inline) {
      case "never": never.push(names[id]); break;
      case "always": always.push(names[id]); break;
      default: heuristic.push(names[id]); break;
    }
  }
  const declarations = [];
  function declare(name, hint) {
    const annotation = hint ? `(@binaryen.inline "\\${hint}") ` : "";
    declarations.push(`${annotation}(func $${name})`);
  }

  // Binaryen keeps the first identical definition. A call that survives the
  // early inliner must keep the more conservative policy when bodies merge.
  for (const name of never) declare(name, NEVER);
  if (always.length) {
    // Low optimization levels run the early inliner solely for MIR 'always'.
    const hint = hintOnlyPass ? NEVER : null;
    for (const name of heuristic) declare(name, hint);
    const count = kernel?.getNumFunctions() ?? 0;
    for (let i = 0; i < count; i++) {
      declare(binaryen.Function.getName(kernel.getFunctionByIndex(i)), hint);
    }
    for (const name of roundHelpers) declare(name, hint);
    for (const name of always) declare(name, ALWAYS);
  }

  return declarations.length
    ? binaryen.parseText(`(module\n${declarations.join("\n")}\n)`)
    : new binaryen.Module();
}

export function defineFunction(module, signatureTypes, name, params, results, vars, body) {
  const declaration = module.getFunction(name);
  if (!declaration) return module.addFunction(name, params, results, vars, body);

  const key = `${params}:${results}`;
  let signature = signatureTypes.get(key);
  if (signature === undefined) {
    const types = new binaryen.TypeBuilder(1);
    types.setSignatureType(0, params, results);
    [signature] = types.buildAndDispose();
    signatureTypes.set(key, signature);
  }
  binaryen._BinaryenFunctionSetType(declaration, signature);
  for (const type of vars) binaryen._BinaryenFunctionAddVar(declaration, type);
  binaryen.Function.setBody(declaration, body);
  return declaration;
}
