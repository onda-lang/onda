import binaryen from "binaryen";
import { PROCESSOR_EXECUTION_INPUT_REJECTED } from "@onda-lang/processor-abi";

// Emit preflight separately from transfer so rejection cannot change workspace,
// state, or existing output records. All generated metadata is per tensor.
export function prepareEventInput(compiler, plan) {
  const m = compiler.module;
  const locals = [];
  const statements = [];
  const c32 = (value) => m.i32.const(value);
  const c64 = (value) => m.i64.const(BigInt.asIntN(64, BigInt(value)));
  const allocate = (type, value = null) => {
    const index = 8 + locals.length;
    locals.push(type);
    if (value !== null) statements.push(m.local.set(index, value));
    return { get: () => m.local.get(index, type), set: (value) => m.local.set(index, value) };
  };
  const require = (condition) => statements.push(m.if(m.i32.eqz(condition), m.return(c32(PROCESSOR_EXECUTION_INPUT_REJECTED))));
  const wide = (value) => m.i64.extend_u(value);
  const narrow = (value) => m.i32.wrap(value);
  const align = (value, alignment) => m.i64.and(m.i64.add(value, c64(alignment - 1)), c64(-alignment));
  const descriptor = () => m.local.get(0, binaryen.i32);
  const memoryBytes = () => m.i64.shl(wide(m.memory.size()), c64(16));
  require(m.i32.and(m.i32.ne(descriptor(), c32(0)), m.i64.le_u(m.i64.add(wide(descriptor()), c64(16)), memoryBytes())));
  const input = allocate(binaryen.i32, m.i32.load(0, 4, descriptor()));
  const inputBytes = allocate(binaryen.i64, wide(m.i32.load(4, 4, descriptor())));
  const workspace = allocate(binaryen.i32, m.i32.load(8, 4, descriptor()));
  const capacity = allocate(binaryen.i64, wide(m.i32.load(12, 4, descriptor())));
  require(m.i64.le_u(inputBytes.get(), c64(0x7fffffff)));
  require(m.i64.le_u(m.i64.add(wide(input.get()), inputBytes.get()), memoryBytes()));
  require(m.i64.le_u(m.i64.add(wide(workspace.get()), capacity.get()), memoryBytes()));
  require(m.i32.or(m.i64.eqz(inputBytes.get()), m.i32.ne(input.get(), c32(0))));
  const wire = allocate(binaryen.i64, c64(0));
  const prepared = allocate(binaryen.i64, c64(0));
  const transfers = [];
  const retain = (encoding, elements, domain = null) => {
    transfers.push({ encoding, domain,
      source: allocate(binaryen.i32, m.i32.add(input.get(), narrow(wire.get()))),
      target: allocate(binaryen.i32, m.i32.add(workspace.get(), narrow(prepared.get()))),
      elements: allocate(binaryen.i64, elements),
    });
  };
  for (const parameter of plan.parameters) {
    let len;
    if (parameter.dynamic) {
      require(m.i64.le_u(m.i64.add(wire.get(), c64(4)), inputBytes.get()));
      len = allocate(binaryen.i64, wide(m.i32.load(0, 1, m.i32.add(input.get(), narrow(wire.get())))));
      require(m.i64.le_u(len.get(), c64(0x7fffffff)));
      statements.push(prepared.set(align(prepared.get(), 4)));
      retain("i32", c64(1));
      statements.push(wire.set(m.i64.add(wire.get(), c64(4))), prepared.set(m.i64.add(prepared.get(), c64(4))));
    } else {
      len = { get: () => c64(1) };
    }
    for (let index = parameter.start; index < parameter.end; index += 1) {
      const tensor = plan.tensors[index];
      statements.push(prepared.set(align(prepared.get(), tensor.size)));
      retain(tensor.encoding, m.i64.mul(len.get(), c64(tensor.elements)), tensor.domain);
      const transfer = transfers.at(-1);
      require(m.i64.le_u(transfer.elements.get(), c64(Math.floor(0x7fffffff / tensor.size))));
      const bytes = () => m.i64.mul(transfer.elements.get(), c64(tensor.size));
      statements.push(wire.set(m.i64.add(wire.get(), bytes())), prepared.set(m.i64.add(prepared.get(), bytes())));
      require(m.i64.le_u(wire.get(), inputBytes.get()));
      require(m.i64.le_u(wire.get(), c64(0x7fffffff)));
      require(m.i64.le_u(prepared.get(), c64(0x7fffffff)));
    }
  }
  require(m.i64.eq(wire.get(), inputBytes.get()));
  require(m.i64.le_u(prepared.get(), capacity.get()));
  require(m.i32.or(m.i64.eqz(prepared.get()), m.i32.and(m.i32.ne(workspace.get(), c32(0)), m.i32.eqz(m.i32.and(workspace.get(), c32(7))))));
  const index = allocate(binaryen.i32);
  const integer = allocate(binaryen.i64);
  const residue = allocate(binaryen.i64);
  const sizes = { bool: 1, i32: 4, f32: 4, i64: 8, f64: 8 };
  for (let id = 0; id < transfers.length; id += 1) {
    const transfer = transfers[id];
    const { encoding, domain } = transfer;
    const size = sizes[encoding];
    const source = () => m.i32.add(transfer.source.get(), m.i32.mul(index.get(), c32(size)));
    const target = () => m.i32.add(transfer.target.get(), m.i32.mul(index.get(), c32(size)));
    let copy;
    if (domain === null && encoding !== "bool") {
      copy = m.memory.copy(transfer.target.get(), transfer.source.get(), narrow(m.i64.mul(transfer.elements.get(), c64(size))));
    } else {
      const loop = `$input.tensor.${id}`;
      const body = [];
      if (encoding === "bool") {
        body.push(m.i32.store8(0, 1, target(), m.i32.ne(m.i32.load8_u(0, 1, source()), c32(0))));
      } else {
        body.push(integer.set(encoding === "i32" ? m.i64.extend_s(m.i32.load(0, 1, source())) : m.i64.load(0, 1, source())));
        if (!domain.wrap) {
          body.push(integer.set(m.select(m.i64.lt_s(integer.get(), c64(domain.min)), c64(domain.min), integer.get(), binaryen.i64)));
          body.push(integer.set(m.select(m.i64.gt_s(integer.get(), c64(domain.max)), c64(domain.max), integer.get(), binaryen.i64)));
        } else {
          const width = domain.max - domain.min + 1n;
          if (width !== 1n << 64n) {
            const minResidue = (domain.min + (1n << 63n)) % width;
            body.push(residue.set(m.i64.rem_u(m.i64.xor(integer.get(), c64(1n << 63n)), c64(width))));
            body.push(integer.set(m.i64.add(c64(domain.min), m.select(m.i64.ge_u(residue.get(), c64(minResidue)),
              m.i64.sub(residue.get(), c64(minResidue)),
              m.i64.sub(c64(width), m.i64.sub(c64(minResidue), residue.get())), binaryen.i64))));
          }
        }
        body.push(encoding === "i32" ? m.i32.store(0, 4, target(), narrow(integer.get())) : m.i64.store(0, 8, target(), integer.get()));
      }
      body.push(index.set(m.i32.add(index.get(), c32(1))), m.br(loop, m.i64.lt_u(wide(index.get()), transfer.elements.get())));
      copy = m.block(null, [index.set(c32(0)), m.loop(loop, m.block(null, body))]);
    }
    statements.push(m.if(m.i64.ne(transfer.elements.get(), c64(0)), copy));
  }
  return { statements, locals, workspace: workspace.get };
}
