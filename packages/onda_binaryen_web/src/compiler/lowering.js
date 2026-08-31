import binaryen from "binaryen";
import { MirCompilerCore } from "./core.js";
import {
  DELEGATE_RECORD_HEADER_SIZE,
  DELEGATE_BATCH_STORAGE_OFFSET,
  DELEGATE_BATCH_CAPACITY_OFFSET,
  DELEGATE_BATCH_USED_OFFSET,
  DELEGATE_BATCH_RECORD_COUNT_OFFSET,
  DELEGATE_BATCH_OVERFLOW_OFFSET,
  PRINT_RECORD_HEADER_SIZE,
  PRINT_BATCH_STORAGE_OFFSET,
  PRINT_BATCH_CAPACITY_OFFSET,
  PRINT_BATCH_USED_OFFSET,
  PRINT_BATCH_RECORD_COUNT_OFFSET,
  PRINT_BATCH_OVERFLOW_OFFSET,
  INIT_ALL_GLOBAL,
  POINTER_GLOBALS,
} from "./shared.js";

export class MirCompilerLowering extends MirCompilerCore {
  compileBlock(block, context) {
    return this.module.block(
      null,
      block.statements.map((statement) =>
        this.compileStatement(statement, context),
      ),
    );
  }

  compileStatement(statement, context) {
    const kind = statement.kind?.kind;
    const data = statement.kind?.data;
    switch (kind) {
      case "assign": {
        if (data.value?.kind === "process_frame") {
          const localId =
            data.destination?.base?.kind === "local" &&
            data.destination.projections.length === 0
              ? data.destination.base.data
              : null;
          if (!context.processFrameLocals.has(localId)) {
            this.fail(
              "process_frame must be the unique definition of an unprojected local",
            );
          }
        }
        if (this.type(this.placeTypeId(data.destination, context)).kind === "slice") {
          return this.storeSlicePlace(
            data.destination,
            this.compileSliceRvalue(data.value, context),
            context,
          );
        }
        const scalar = this.placeScalarType(data.destination, context);
        const value = this.compileRvalue(data.value, scalar, context);
        return this.storePlace(data.destination, value, scalar, context);
      }
      case "call":
        return this.compileCall(data, context);
      case "publish_delegate":
        return this.compilePublishDelegate(data, context);
      case "publish_log":
        return this.compilePublishLog(data, context);
      case "output_store":
        return this.compileOutputStore(data, context);
      case "control_output_store":
        return this.compileControlOutputStore(data, context);
      case "if":
        return this.module.if(
          this.compileValue(data.condition, context),
          this.compileBlock(data.then_block, context),
          this.compileBlock(data.else_block, context),
        );
      case "loop": {
        const id = this.nextLabel++;
        const breakLabel = `$onda.break.${id}`;
        const continueLabel = `$onda.loop.${id}`;
        context.breakLabels.push(breakLabel);
        context.continueLabels.push(continueLabel);
        const body = this.compileBlock(data.body, context);
        context.breakLabels.pop();
        context.continueLabels.pop();
        return this.module.block(breakLabel, [
          this.module.loop(
            continueLabel,
            this.module.block(null, [body, this.module.br(continueLabel)]),
          ),
        ]);
      }
      case "break":
        return this.module.br(this.currentLabel(context.breakLabels, "break"));
      case "continue":
        return this.module.br(
          this.currentLabel(context.continueLabels, "continue"),
        );
      case "return": {
        const values = data.values.map((value) =>
          this.compileValue(value, context),
        );
        return this.module.return(
          values.length > 1 ? this.module.tuple.make(values) : values[0],
        );
      }
      case "buffer_store":
        return this.compileBufferStore(data, context);
      case "buffer_param_store":
        return this.compileBufferParamStore(data, context);
      case "slice_store":
        return this.compileSliceStore(data, context);
      case "slice_fill":
        return this.compileSliceFill(statement, data, context);
      case "slice_copy":
        return this.compileSliceCopy(statement, data, context);
      default:
        this.fail(`unknown MIR statement '${String(kind)}'`);
    }
  }

  compileCall(data, context) {
    this.requireFunctionId(data.function, "call target");
    const target = this.mir.functions[data.function];
    if (data.args.length !== target.params.length) {
      this.fail(`call to '${target.name}' has the wrong number of arguments`);
    }
    const args = data.args.flatMap((argument, index) => {
      const parameterType = this.type(target.params[index].ty);
      if (parameterType.kind === "scalar") {
        const passingMode = this.parameterPassingMode(data.function, index);
        if (passingMode === "value") {
          if (target.params[index].mode !== "value") {
            if (argument.kind !== "place") {
              this.fail(
                `promoted scalar reference argument ${index} of '${target.name}' is not a place`,
              );
            }
            return [this.loadPlace(argument.data, context)];
          }
          if (argument.kind !== "value") {
            this.fail(`scalar call argument ${index} of '${target.name}' is not a value`);
          }
          return [this.compileValue(argument.data, context)];
        }
        if (!["place", "slice_element"].includes(argument.kind)) {
          this.fail(
            `reference call argument ${index} of '${target.name}' is not addressable`,
          );
        }
        return [
          argument.kind === "place"
            ? this.placeAddress(argument.data, context)
            : this.compileSliceAddress(
                argument.data.slice,
                argument.data.index,
                argument.data.bounds,
                context,
                target.params[index].mode === "read_write_reference",
              ),
        ];
      }
      if (parameterType.kind === "slice") {
        if (argument.kind !== "value") {
          this.fail(`slice call argument ${index} of '${target.name}' is not a value`);
        }
        return this.compileSliceValue(argument.data, context);
      }
      if (parameterType.kind === "array") {
        if (
          target.params[index].mode === "value" ||
          !["place", "array_window", "slice_window"].includes(argument.kind)
        ) {
          this.fail(`array reference argument ${index} of '${target.name}' is invalid`);
        }
        if (argument.kind === "place") {
          return [this.placeAddress(argument.data, context)];
        }
        if (argument.kind === "array_window") {
          return [
            this.compileArrayWindowAddress(
              argument.data,
              parameterType,
              context,
            ),
          ];
        }
        return [
            this.compileSliceWindowAddress(
              argument.data,
              parameterType,
              context,
              target.params[index].mode === "read_write_reference",
            ),
        ];
      }
      if (parameterType.kind === "buffer") {
        if (argument.kind === "buffer") {
          return this.compileInterfaceBufferValue(argument.data, context);
        }
        if (argument.kind === "buffer_param") {
          return this.loadBufferParamValue(argument.data, context);
        }
        if (argument.kind === "place") {
          return this.loadBufferPlace(argument.data, context);
        }
        this.fail(`buffer call argument ${index} of '${target.name}' is invalid`);
      }
      if (parameterType.kind === "buffer_span") {
        if (argument.kind !== "buffer_span") {
          this.fail(`buffer span call argument ${index} of '${target.name}' is invalid`);
        }
        return this.compileBufferSpanValue(argument.data, parameterType, context);
      }
      this.fail(
        `call argument ${index} of '${target.name}' has unsupported type '${parameterType.kind}'`,
      );
    });
    if (data.results.length !== target.results.length) {
      this.fail(`call result arity for '${target.name}' does not match its signature`);
    }
    const resultScalars = target.results.map((result, resultId) =>
      this.requireScalarType(result, `result ${resultId} of '${target.name}'`),
    );
    const resultType = this.wasmResultType(resultScalars);
    const call = this.module.call(this.functionNames[data.function], args, resultType);
    const localReferenceSync = data.args.flatMap((argument, index) => {
      const parameter = target.params[index];
      if (
        this.parameterPassingMode(data.function, index) === "value"
        || argument.kind !== "place"
        || argument.data.base.kind !== "local"
        || argument.data.projections.length !== 0
      ) {
        return [];
      }
      const layout =
        this.localScalarRefLayout[context.functionId]?.[argument.data.base.data];
      if (!layout) return [];
      return [{
        localId: argument.data.base.data,
        address: layout.address,
        scalar: layout.scalar,
        writeBack: parameter.mode === "read_write_reference",
      }];
    });
    const beforeCall = localReferenceSync.map((sync) =>
      this.storeScalar(
        sync.scalar,
        this.module.i32.const(sync.address),
        this.module.local.get(
          this.localIndex(sync.localId, context),
          this.wasmType(sync.scalar),
        ),
      ),
    );
    const afterCall = localReferenceSync
      .filter((sync) => sync.writeBack)
      .map((sync) =>
        this.module.local.set(
          this.localIndex(sync.localId, context),
          this.loadScalar(sync.scalar, this.module.i32.const(sync.address)),
        ),
      );
    const resultSpill = context.callResultLocals.get(data);
    if (localReferenceSync.length > 0 && data.results.length > 0) {
      if (!resultSpill) {
        this.fail(`internal result spill is missing for call to '${target.name}'`);
      }
      const spilledValue = () =>
        this.module.local.get(resultSpill.index, resultSpill.type);
      const assignResults = data.results.length === 1
        ? [
            this.module.local.set(
              this.localIndex(data.results[0], context),
              spilledValue(),
            ),
          ]
        : data.results.map((localId, index) =>
            this.module.local.set(
              this.localIndex(localId, context),
              this.module.tuple.extract(spilledValue(), index),
            ),
          );
      return this.module.block(null, [
        ...beforeCall,
        this.module.local.set(resultSpill.index, call),
        ...afterCall,
        ...assignResults,
        ...this.propagateRuntimeFailure(data.function, context),
      ]);
    }
    let compiledCall;
    if (data.results.length === 0) {
      compiledCall = call;
    } else if (data.results.length === 1) {
      compiledCall = this.module.local.set(
        this.localIndex(data.results[0], context),
        call,
      );
    } else {
      const tupleLocal = context.callResultLocals.get(data);
      if (!tupleLocal) {
        this.fail(`internal tuple spill is missing for call to '${target.name}'`);
      }
      const tupleValue = () =>
        this.module.local.get(tupleLocal.index, tupleLocal.type);
      compiledCall = this.module.block(null, [
        this.module.local.set(tupleLocal.index, call),
        ...data.results.map((localId, index) =>
          this.module.local.set(
            this.localIndex(localId, context),
            this.module.tuple.extract(tupleValue(), index),
          ),
        ),
      ]);
    }
    if (localReferenceSync.length === 0) {
      const propagation = this.propagateRuntimeFailure(data.function, context);
      return propagation.length === 0
        ? compiledCall
        : this.module.block(null, [compiledCall, ...propagation]);
    }
    return this.module.block(null, [
      ...beforeCall,
      compiledCall,
      ...afterCall,
      ...this.propagateRuntimeFailure(data.function, context),
    ]);
  }

  compilePublishDelegate(data, context) {
    const delegate = this.mir.interface.delegates[data.delegate];
    const layout = this.delegateLayout[data.delegate];
    if (!delegate || !layout || data.args?.length !== delegate.params.length) {
      this.fail(`delegate id ${String(data.delegate)} has an invalid publication payload`);
    }
    if (context.function.attributes.runtime_context !== true) {
      this.fail(`function '${context.function.name}' publishes without runtime context`);
    }

    const payloadBytes = this.allocateGeneratedLocal(
      context,
      "i32",
      `delegate.${data.delegate}.payload_bytes`,
    );
    const oversized = this.allocateGeneratedLocal(
      context,
      "i32",
      `delegate.${data.delegate}.oversized`,
    );
    const record = this.allocateGeneratedLocal(
      context,
      "i32",
      `delegate.${data.delegate}.record`,
    );
    const cursor = this.allocateGeneratedLocal(
      context,
      "i32",
      `delegate.${data.delegate}.cursor`,
    );
    const counter = this.allocateGeneratedLocal(
      context,
      "i32",
      `delegate.${data.delegate}.copy_index`,
    );
    const payload = () => this.module.local.get(payloadBytes, binaryen.i32);
    const tooLarge = () => this.module.local.get(oversized, binaryen.i32);
    const batch = () =>
      this.module.global.get(POINTER_GLOBALS.delegateBatch, binaryen.i32);
    const validationStatements = [];
    const collectionStatements = [
      this.module.local.set(
        payloadBytes,
        this.module.i32.const(layout.minimumByteLength),
      ),
      this.module.local.set(
        oversized,
        this.module.i32.const(
          layout.minimumByteLength > 0xffff_ffff - DELEGATE_RECORD_HEADER_SIZE
            ? 1
            : 0,
        ),
      ),
    ];

    for (const [paramId, param] of delegate.params.entries()) {
      const type = this.type(param.ty);
      const argument = data.args[paramId];
      if (argument?.kind !== "value") {
        this.fail(`delegate '${delegate.name}' argument ${paramId} is not a value`);
      }
      if (type.kind !== "slice") continue;
      const element = type.data.element;
      const elementSize = this.scalarSize(element);
      const length = () => this.compileSliceValue(argument.data, context)[2];
      const delta = () =>
        this.module.i32.mul(length(), this.module.i32.const(elementSize));
      const next = () => this.module.i32.add(payload(), delta());
      collectionStatements.push(
        this.module.local.set(
          oversized,
          this.module.i32.or(
            tooLarge(),
            this.module.i32.or(
              this.module.i32.gt_u(
                length(),
                this.module.i32.const(Math.floor(0xffff_ffff / elementSize)),
              ),
              this.module.i32.lt_u(next(), payload()),
            ),
          ),
        ),
        this.module.local.set(payloadBytes, next()),
      );
    }
    for (const [paramId, param] of delegate.params.entries()) {
      const type = this.type(param.ty);
      if (type.kind !== "array") continue;
      const argument = data.args[paramId];
      const sliceLength = () => this.compileSliceValue(argument.data, context)[2];
      validationStatements.push(
        this.module.if(
          this.module.i32.ne(
            sliceLength(),
            this.module.i32.const(type.data.len),
          ),
          this.raiseRuntimeFailure(context),
        ),
      );
    }
    collectionStatements.push(
      this.module.local.set(
        oversized,
        this.module.i32.or(
          tooLarge(),
          this.module.i32.gt_u(
            payload(),
            this.module.i32.const(0xffff_ffff - DELEGATE_RECORD_HEADER_SIZE),
          ),
        ),
      ),
      this.compileDelegateBatchAppend(
        data.delegate,
        delegate,
        data.args,
        payload,
        tooLarge,
        record,
        cursor,
        counter,
        context,
      ),
    );
    return this.module.block(null, [
      ...validationStatements,
      this.module.if(
        this.module.i32.ne(batch(), this.module.i32.const(0)),
        this.module.block(null, collectionStatements),
      ),
    ]);
  }

  compilePublishLog(data, context) {
    const site = this.mir.log_sites?.[data.site];
    const args = data.arguments;
    if (!site || !Array.isArray(args) || args.length !== site.argument_types?.length) {
      this.fail(`log site id ${String(data.site)} has an invalid publication payload`);
    }
    if (context.function.attributes.runtime_context !== true) {
      this.fail(`function '${context.function.name}' prints without runtime context`);
    }

    const payloadSize = site.payload_size;
    const calculatedSize = site.argument_types.reduce(
      (size, scalar) => size + this.scalarSize(scalar),
      0,
    );
    if (!Number.isInteger(payloadSize) || payloadSize !== calculatedSize) {
      this.fail(`log site id ${data.site} has an invalid payload size`);
    }

    const storageLocal = this.allocateGeneratedLocal(
      context,
      "i32",
      `log.${data.site}.storage`,
    );
    const capacityLocal = this.allocateGeneratedLocal(
      context,
      "i32",
      `log.${data.site}.capacity`,
    );
    const usedLocal = this.allocateGeneratedLocal(
      context,
      "i32",
      `log.${data.site}.used`,
    );
    const recordLocal = this.allocateGeneratedLocal(
      context,
      "i32",
      `log.${data.site}.record`,
    );
    const cursorLocal = this.allocateGeneratedLocal(
      context,
      "i32",
      `log.${data.site}.cursor`,
    );
    const sequenceLocal = this.allocateGeneratedLocal(
      context,
      "i32",
      `log.${data.site}.sequence`,
    );
    const batch = () =>
      this.module.global.get(POINTER_GLOBALS.printBatch, binaryen.i32);
    const field = (offset) =>
      this.module.i32.add(batch(), this.module.i32.const(offset));
    const local = (id) => this.module.local.get(id, binaryen.i32);
    const requiredSize = payloadSize + PRINT_RECORD_HEADER_SIZE;
    const overflowAddress = () => field(PRINT_BATCH_OVERFLOW_OFFSET);
    const overflow = () => this.module.i32.load(0, 4, overflowAddress());
    const drop = this.module.i32.store(
      0,
      4,
      overflowAddress(),
      this.module.select(
        this.module.i32.eq(overflow(), this.module.i32.const(-1)),
        overflow(),
        this.module.i32.add(overflow(), this.module.i32.const(1)),
      ),
    );
    const fits = this.module.i32.and(
      this.module.i32.le_u(local(usedLocal), local(capacityLocal)),
      this.module.i32.le_u(
        this.module.i32.const(requiredSize),
        this.module.i32.sub(local(capacityLocal), local(usedLocal)),
      ),
    );
    const record = () => local(recordLocal);
    const cursor = () => local(cursorLocal);
    const write = [
      this.module.local.set(
        recordLocal,
        this.module.i32.add(local(storageLocal), local(usedLocal)),
      ),
      this.module.i32.store(0, 1, record(), this.module.i32.const(data.site)),
      this.module.i32.store(4, 1, record(), this.module.i32.const(payloadSize)),
      this.module.i32.store(8, 1, record(), local(sequenceLocal)),
      this.module.local.set(
        cursorLocal,
        this.module.i32.add(record(), this.module.i32.const(PRINT_RECORD_HEADER_SIZE)),
      ),
    ];
    for (const [index, scalar] of site.argument_types.entries()) {
      write.push(
        this.storePackedScalar(
          scalar,
          cursor(),
          this.compileValue(args[index], context),
        ),
        this.module.local.set(
          cursorLocal,
          this.module.i32.add(cursor(), this.module.i32.const(this.scalarSize(scalar))),
        ),
      );
    }
    const usedAddress = () => field(PRINT_BATCH_USED_OFFSET);
    const countAddress = () => field(PRINT_BATCH_RECORD_COUNT_OFFSET);
    write.push(
      this.module.i32.store(
        0,
        4,
        usedAddress(),
        this.module.i32.add(local(usedLocal), this.module.i32.const(requiredSize)),
      ),
      this.module.i32.store(
        0,
        4,
        countAddress(),
        this.module.i32.add(
          this.module.i32.load(0, 4, countAddress()),
          this.module.i32.const(1),
        ),
      ),
    );
    return this.module.if(
      this.module.i32.ne(batch(), this.module.i32.const(0)),
      this.module.block(null, [
        this.advanceOutputSequence(sequenceLocal),
        this.module.local.set(
          storageLocal,
          this.module.i32.load(0, 4, field(PRINT_BATCH_STORAGE_OFFSET)),
        ),
        this.module.if(
          this.module.i32.ne(local(storageLocal), this.module.i32.const(0)),
          this.module.block(null, [
            this.module.local.set(
              capacityLocal,
              this.module.i32.load(0, 4, field(PRINT_BATCH_CAPACITY_OFFSET)),
            ),
            this.module.local.set(
              usedLocal,
              this.module.i32.load(0, 4, field(PRINT_BATCH_USED_OFFSET)),
            ),
            this.module.if(fits, this.module.block(null, write), drop),
          ]),
        ),
      ]),
    );
  }

  compileDelegateBatchAppend(
    delegateId,
    delegate,
    args,
    payload,
    tooLarge,
    recordLocal,
    cursorLocal,
    counterLocal,
    context,
  ) {
    const batch = () =>
      this.module.global.get(POINTER_GLOBALS.delegateBatch, binaryen.i32);
    const field = (offset) =>
      this.module.i32.add(batch(), this.module.i32.const(offset));
    const storage = this.allocateGeneratedLocal(
      context,
      "i32",
      `delegate.${delegateId}.storage`,
    );
    const capacity = this.allocateGeneratedLocal(
      context,
      "i32",
      `delegate.${delegateId}.capacity`,
    );
    const used = this.allocateGeneratedLocal(
      context,
      "i32",
      `delegate.${delegateId}.used`,
    );
    const required = this.allocateGeneratedLocal(
      context,
      "i32",
      `delegate.${delegateId}.required`,
    );
    const sequence = this.allocateGeneratedLocal(
      context,
      "i32",
      `delegate.${delegateId}.sequence`,
    );
    const local = (id) => this.module.local.get(id, binaryen.i32);
    const write = this.compileDelegateRecord(
      delegateId,
      delegate,
      args,
      payload,
      () => local(storage),
      () => local(used),
      recordLocal,
      cursorLocal,
      counterLocal,
      sequence,
      context,
    );
    const overflowAddress = () => field(DELEGATE_BATCH_OVERFLOW_OFFSET);
    const overflow = () => this.module.i32.load(0, 4, overflowAddress());
    const drop = this.module.i32.store(
      0,
      4,
      overflowAddress(),
      this.module.select(
        this.module.i32.eq(overflow(), this.module.i32.const(-1)),
        overflow(),
        this.module.i32.add(overflow(), this.module.i32.const(1)),
      ),
    );
    const fit = this.module.i32.and(
      this.module.i32.eq(tooLarge(), this.module.i32.const(0)),
      this.module.i32.and(
        this.module.i32.le_u(local(used), local(capacity)),
        this.module.i32.le_u(
          local(required),
          this.module.i32.sub(local(capacity), local(used)),
        ),
      ),
    );
    return this.module.block(null, [
      this.advanceOutputSequence(sequence),
      this.module.local.set(
        storage,
        this.module.i32.load(0, 4, field(DELEGATE_BATCH_STORAGE_OFFSET)),
      ),
      this.module.if(
        this.module.i32.ne(local(storage), this.module.i32.const(0)),
        this.module.block(null, [
          this.module.local.set(
            capacity,
            this.module.i32.load(0, 4, field(DELEGATE_BATCH_CAPACITY_OFFSET)),
          ),
          this.module.local.set(
            used,
            this.module.i32.load(0, 4, field(DELEGATE_BATCH_USED_OFFSET)),
          ),
          this.module.local.set(
            required,
            this.module.i32.add(
              payload(),
              this.module.i32.const(DELEGATE_RECORD_HEADER_SIZE),
            ),
          ),
          this.module.if(fit, write, drop),
        ]),
      ),
    ]);
  }

  compileDelegateRecord(
    delegateId,
    delegate,
    args,
    payload,
    storage,
    used,
    recordLocal,
    cursorLocal,
    counterLocal,
    sequenceLocal,
    context,
  ) {
    const batch = () =>
      this.module.global.get(POINTER_GLOBALS.delegateBatch, binaryen.i32);
    const record = () => this.module.local.get(recordLocal, binaryen.i32);
    const cursor = () => this.module.local.get(cursorLocal, binaryen.i32);
    const statements = [];
    statements.push(
      this.module.local.set(recordLocal, this.module.i32.add(storage(), used())),
      this.module.i32.store(0, 1, record(), this.module.i32.const(delegateId)),
      this.module.i32.store(4, 1, record(), payload()),
      this.module.i32.store(
        8,
        1,
        record(),
        this.module.local.get(sequenceLocal, binaryen.i32),
      ),
      this.module.local.set(
        cursorLocal,
        this.module.i32.add(
          record(),
          this.module.i32.const(DELEGATE_RECORD_HEADER_SIZE),
        ),
      ),
    );
    for (const [paramId, param] of delegate.params.entries()) {
      const type = this.type(param.ty);
      const argument = args[paramId];
      if (type.kind === "scalar") {
        statements.push(
          this.storePackedScalar(
            type.data,
            cursor(),
            this.compileValue(argument.data, context),
          ),
          this.module.local.set(
            cursorLocal,
            this.module.i32.add(
              cursor(),
              this.module.i32.const(this.scalarSize(type.data)),
            ),
          ),
        );
        continue;
      }
      const element = type.kind === "slice"
        ? type.data.element
        : this.requireScalarType(
          type.data.element,
          `delegate '${delegate.name}' aggregate parameter ${paramId}`,
        );
      const slice = () => this.compileSliceValue(argument.data, context);
      const count = type.kind === "array"
        ? () => this.module.i32.const(type.data.len)
        : () => slice()[2];
      if (type.kind === "slice") {
        statements.push(
          this.module.i32.store(0, 1, cursor(), count()),
          this.module.local.set(
            cursorLocal,
            this.module.i32.add(cursor(), this.module.i32.const(4)),
          ),
        );
      }
      statements.push(
        this.compilePackedSliceCopy(
          slice,
          count,
          element,
          cursor,
          counterLocal,
        ),
        this.module.local.set(
          cursorLocal,
          this.module.i32.add(
            cursor(),
            this.module.i32.mul(
              count(),
              this.module.i32.const(this.scalarSize(element)),
            ),
          ),
        ),
      );
    }
    const usedAddress = () =>
      this.module.i32.add(
        batch(),
        this.module.i32.const(DELEGATE_BATCH_USED_OFFSET),
      );
    const countAddress = () =>
      this.module.i32.add(
        batch(),
        this.module.i32.const(DELEGATE_BATCH_RECORD_COUNT_OFFSET),
      );
    statements.push(
      this.module.i32.store(
        0,
        4,
        usedAddress(),
        this.module.i32.add(
          used(),
          this.module.i32.add(
            payload(),
            this.module.i32.const(DELEGATE_RECORD_HEADER_SIZE),
          ),
        ),
      ),
      this.module.i32.store(
        0,
        4,
        countAddress(),
        this.module.i32.add(
          this.module.i32.load(0, 4, countAddress()),
          this.module.i32.const(1),
        ),
      ),
    );
    return this.module.block(null, statements);
  }

  compilePackedSliceCopy(slice, count, scalar, destination, counterLocal) {
    const loopLabel = `$onda.delegate.copy.${this.nextLabel++}`;
    const counter = () => this.module.local.get(counterLocal, binaryen.i32);
    const sourceAddress = () =>
      this.module.i32.add(
        slice()[0],
        this.module.i32.mul(counter(), slice()[3]),
      );
    const destinationAddress = () =>
      this.module.i32.add(
        destination(),
        this.module.i32.mul(
          counter(),
          this.module.i32.const(this.scalarSize(scalar)),
        ),
      );
    return this.module.block(null, [
      this.module.local.set(counterLocal, this.module.i32.const(0)),
      this.module.loop(
        loopLabel,
        this.module.if(
          this.module.i32.lt_u(counter(), count()),
          this.module.block(null, [
            this.storePackedScalar(
              scalar,
              destinationAddress(),
              this.loadScalar(scalar, sourceAddress()),
            ),
            this.module.local.set(
              counterLocal,
              this.module.i32.add(counter(), this.module.i32.const(1)),
            ),
            this.module.br(loopLabel),
          ]),
        ),
      ),
    ]);
  }

  compileOutputStore(data, context) {
    this.requireProcessFrame(data.frame, context, "audio output store");
    const port = this.outputLayout[data.output];
    if (!port) {
      this.fail(`output id ${data.output} is out of range`);
    }
    const channelPointer = this.audioChannelPointer(
      POINTER_GLOBALS.outputs,
      port,
      data.element,
      data.bounds,
      context,
    );
    const sampleAddress = this.module.i32.add(
      channelPointer,
      this.module.i32.mul(
        this.compileValue(data.frame, context),
        this.module.i32.const(port.size),
      ),
    );
    return this.storeScalar(
      port.scalar,
      sampleAddress,
      this.compileValue(data.value, context),
    );
  }

  compileControlOutputStore(data, context) {
    const output = this.mir.interface.control_outputs[data.output];
    const layout = this.controlOutputLayout[data.output];
    if (!output || !layout) {
      this.fail(`control output id ${data.output} is out of range`);
    }
    const flattened = this.flattenPortType(this.type(output.ty));
    let elementOffset = this.module.i32.const(0);
    if (this.type(output.ty).kind !== "array") {
      if (data.element !== null) {
        this.fail("scalar control output unexpectedly has an element index");
      }
    } else {
      if (data.element === null) {
        this.fail("array control output is missing its element index");
      }
      const index = this.compileBoundedIndex(
        data.element,
        flattened.channels,
        data.bounds,
        context,
      );
      elementOffset = this.module.i32.mul(
        index,
        this.module.i32.const(layout.size / flattened.channels),
      );
    }
    const address = this.module.i32.add(
      this.module.i32.add(
        this.module.global.get(POINTER_GLOBALS.state, binaryen.i32),
        this.module.i32.const(layout.offset),
      ),
      elementOffset,
    );
    return this.storeScalar(
      flattened.scalar,
      address,
      this.compileValue(data.value, context),
    );
  }

  compileBufferStore(data, context) {
    const buffer = this.requireBufferRef(data.buffer);
    if (buffer.access !== "read_write") {
      this.fail(`buffer '${buffer.name}' is read-only`);
    }
    return this.storeScalar(
      buffer.element,
      this.compileBufferAddress(data, context, true),
      this.compileValue(data.value, context),
    );
  }

  compileBufferParamStore(data, context) {
    const type = this.bufferParamType(data.parameter, context);
    if (type.data.access !== "read_write") {
      this.fail(`buffer parameter ${data.parameter} is read-only`);
    }
    return this.storeScalar(
      type.data.element,
      this.compileBufferParamAddress(data, context, true),
      this.compileValue(data.value, context),
    );
  }

  compileRvalue(rvalue, expectedScalar, context) {
    const kind = rvalue.kind;
    const data = rvalue.data;
    switch (kind) {
      case "use":
        return this.compileValue(data, context);
      case "load":
        return this.loadPlace(data, context);
      case "unary":
        return this.compileUnary(
          data.op,
          this.valueScalarType(data.operand, context),
          this.compileValue(data.operand, context),
        );
      case "binary": {
        const scalar = this.valueScalarType(data.lhs, context);
        return this.compileBinary(
          data.op,
          scalar,
          () => this.compileValue(data.lhs, context),
          () => this.compileValue(data.rhs, context),
          context,
        );
      }
      case "compare": {
        const scalar = this.valueScalarType(data.lhs, context);
        return this.compileCompare(
          data.op,
          scalar,
          this.compileValue(data.lhs, context),
          this.compileValue(data.rhs, context),
        );
      }
      case "cast":
        return this.compileCast(
          this.valueScalarType(data.value, context),
          data.to,
          this.compileValue(data.value, context),
        );
      case "intrinsic":
        return this.compileIntrinsic(data, expectedScalar, context);
      case "init_all":
        if (context.function.kind?.kind !== "init") {
          this.fail("init_all is only valid in the init entry point");
        }
        return this.module.global.get(INIT_ALL_GLOBAL, binaryen.i32);
      case "process_frame":
        return this.compileProcessFrame(data, context);
      case "input_load":
        return this.compileInputLoad(data, context);
      case "output_load":
        return this.compileOutputLoad(data, context);
      case "const_data_load":
        return this.compileConstDataLoad(data, context);
      case "buffer_load":
        return this.compileBufferLoad(data, context);
      case "buffer_param_load":
        return this.compileBufferParamLoad(data, context);
      case "buffer_len":
        return this.compileBufferLen(data, context);
      case "buffer_param_len":
        return this.compileBufferParamLen(data, context);
      case "buffer_channels":
        return this.compileBufferChannels(data, context);
      case "buffer_param_channels":
        return this.compileBufferParamChannels(data, context);
      case "buffer_sample_rate":
        return this.loadBufferTableValue(
          POINTER_GLOBALS.bufferSampleRates,
          data,
          "f32",
          context,
        );
      case "buffer_param_sample_rate":
        return this.loadBufferParamComponent(data, 4, "f32", context);
      case "buffer_is_bound":
        return this.compileBufferIsBound(data, context);
      case "buffer_param_is_bound":
        return this.loadBufferParamComponent(data, 5, "i32", context);
      case "slice_len":
        return this.compileSliceValue(data, context)[2];
      case "slice_load":
        return this.compileSliceLoad(data, context);
      case "make_slice":
        this.fail("make_slice must be assigned to a slice-typed destination");
        break;
      default:
        this.fail(`unknown MIR rvalue '${String(kind)}'`);
    }
  }

  compileSliceRvalue(rvalue, context) {
    switch (rvalue.kind) {
      case "use":
        return this.compileSliceValue(rvalue.data, context);
      case "load":
        return this.loadSlicePlace(rvalue.data, context);
      case "make_slice":
        return this.compileMakeSlice(rvalue.data, context);
      default:
        this.fail(`rvalue '${String(rvalue.kind)}' does not produce a slice`);
    }
  }

  compileInputLoad(data, context) {
    this.requireProcessFrame(data.frame, context, "audio input load");
    const port = this.inputLayout[data.input];
    if (!port) {
      this.fail(`input id ${data.input} is out of range`);
    }
    const channelPointer = this.audioChannelPointer(
      POINTER_GLOBALS.inputs,
      port,
      data.element,
      data.bounds,
      context,
    );
    const sampleAddress = this.module.i32.add(
      channelPointer,
      this.module.i32.mul(
        this.compileValue(data.frame, context),
        this.module.i32.const(port.size),
      ),
    );
    return this.loadScalar(port.scalar, sampleAddress);
  }

  compileProcessFrame(data, context) {
    if (context.function.kind?.kind !== "process") {
      this.fail("process_frame is only valid in the process entry point");
    }
    const offset = () => this.compileValue(data.offset, context);
    const startFrame = () =>
      this.module.local.get(context.paramLayouts[0].index, binaryen.i32);
    const frames = () =>
      this.module.local.get(context.paramLayouts[1].index, binaryen.i32);
    const invalid = this.module.i32.ge_u(offset(), frames());
    return this.module.if(
      invalid,
      this.raiseRuntimeFailure(context),
      this.module.i32.add(startFrame(), offset()),
    );
  }

  requireProcessFrame(value, context, operation) {
    if (
      context.function.kind?.kind !== "process" ||
      value?.kind !== "local" ||
      !context.processFrameLocals.has(value.data)
    ) {
      this.fail(`${operation} frame must come directly from process_frame`);
    }
  }

  compileOutputLoad(data, context) {
    this.requireProcessFrame(data.frame, context, "audio output load");
    const port = this.outputLayout[data.output];
    if (!port) {
      this.fail(`output id ${data.output} is out of range`);
    }
    const channelPointer = this.audioChannelPointer(
      POINTER_GLOBALS.outputs,
      port,
      data.element,
      data.bounds,
      context,
    );
    const sampleAddress = this.module.i32.add(
      channelPointer,
      this.module.i32.mul(
        this.compileValue(data.frame, context),
        this.module.i32.const(port.size),
      ),
    );
    return this.loadScalar(port.scalar, sampleAddress);
  }

  compilePortChannel(port, element, bounds, context) {
    if (!port.isArray) {
      if (element !== null) {
        this.fail("scalar audio port unexpectedly has an element index");
      }
      return this.module.i32.const(port.channel);
    }
    if (element === null) {
      this.fail("array audio port is missing its element index");
    }
    const index = this.compileBoundedIndex(element, port.channels, bounds, context);
    return this.module.i32.add(this.module.i32.const(port.channel), index);
  }

  audioChannelPointer(globalName, port, element, bounds, context) {
    const staticChannel = this.staticPortChannel(port, element, bounds);
    if (staticChannel === null) {
      return this.loadAudioChannelPointer(
        globalName,
        this.compilePortChannel(port, element, bounds, context),
      );
    }
    const key = `${globalName}:${staticChannel}`;
    let local = context.audioChannelPointerCache.get(key);
    if (local === undefined) {
      local = this.allocateGeneratedLocal(
        context,
        "i32",
        `audio.${globalName.slice(6)}.${staticChannel}`,
      );
      context.audioChannelPointerCache.set(key, local);
      context.entryInitializers.push(
        this.module.local.set(
          local,
          this.loadAudioChannelPointer(
            globalName,
            this.module.i32.const(staticChannel),
          ),
        ),
      );
    }
    return this.module.local.get(local, binaryen.i32);
  }

  loadAudioChannelPointer(globalName, channel) {
    const tableAddress = this.module.i32.add(
      this.module.global.get(globalName, binaryen.i32),
      this.module.i32.mul(channel, this.module.i32.const(4)),
    );
    return this.module.i32.load(0, 4, tableAddress);
  }

  staticPortChannel(port, element, bounds) {
    if (!port.isArray) return port.channel;
    if (
      element?.kind !== "constant"
      || element.data?.type !== "i32"
      || !Number.isInteger(element.data.value)
    ) {
      return null;
    }
    let index = element.data.value;
    if (bounds === "clamp") {
      index = Math.min(port.channels - 1, Math.max(0, index));
    } else if (index < 0 || index >= port.channels) {
      return null;
    }
    return port.channel + index;
  }

  compileConstDataLoad(data, context) {
    const item = this.constLayout[data.data];
    if (!item) {
      this.fail(`const data id ${data.data} is out of range`);
    }
    const index = this.compileBoundedIndex(data.index, item.len, data.bounds, context);
    const address = this.module.i32.add(
      this.module.i32.const(item.address),
      this.module.i32.mul(index, this.module.i32.const(this.scalarSize(item.scalar))),
    );
    return this.loadScalar(item.scalar, address);
  }

  compileBufferLoad(data, context) {
    const buffer = this.requireBufferRef(data.buffer);
    return this.loadScalar(
      buffer.element,
      this.compileBufferAddress(data, context, false),
    );
  }

  compileBufferParamLoad(data, context) {
    const type = this.bufferParamType(data.parameter, context);
    return this.loadScalar(
      type.data.element,
      this.compileBufferParamAddress(data, context, false),
    );
  }

  compileBufferParamAddress(data, context, write) {
    const type = this.bufferParamType(data.parameter, context);
    return this.withBufferParamSelector(
      data.parameter,
      context,
      binaryen.i32,
      (selector, staticSelector, prelude) => {
        let frames = this.bufferParamComponentFactory(
          data.parameter,
          2,
          "i32",
          selector,
          staticSelector,
          context,
        );
        let channels = this.bufferParamChannelsFactory(
          data.parameter,
          selector,
          staticSelector,
          context,
        );
        if (data.parameter?.kind === "array_element" && staticSelector === null) {
          frames = this.snapshotOperationValue(
            frames,
            "i32",
            "buffer_param.frames",
            prelude,
            context,
          );
          if (this.bufferChannelMetadata(
            type.data.channels,
            type.data.element,
          ).kind === "dynamic") {
            channels = this.snapshotOperationValue(
              channels,
              "i32",
              "buffer_param.channels",
              prelude,
              context,
            );
          }
        }
        const frame = this.compileDynamicBoundedIndex(
          () => this.compileValue(data.index, context),
          frames,
          data.bounds,
          context,
          true,
        );
        const index = data.channel === null
          ? frame
          : this.module.i32.add(
            this.module.i32.mul(frame, channels()),
            this.compileDynamicBoundedIndex(
              () => this.compileValue(data.channel, context),
              channels,
              data.bounds,
              context,
              true,
            ),
          );
        const pointer = this.bufferParamComponentFactory(
          data.parameter,
          write ? 1 : 0,
          "i32",
          selector,
          staticSelector,
          context,
        );
        return this.bufferPointerWithOffset(
          pointer,
          this.module.i32.mul(
            index,
            this.module.i32.const(this.scalarSize(type.data.element)),
          ),
          write,
          context,
        );
      },
    );
  }

  compileBufferAddress(data, context, write) {
    const buffer = this.requireBufferRef(data.buffer);
    return this.withBufferRefIndex(
      data.buffer,
      context,
      binaryen.i32,
      (descriptorIndex, staticIndex, prelude) => {
        let frames = this.bufferTableValueFactory(
          POINTER_GLOBALS.bufferFrames,
          descriptorIndex,
          staticIndex,
          "i32",
          context,
        );
        let channels = this.bufferChannelsFactory(
          data.buffer,
          descriptorIndex,
          staticIndex,
          context,
        );
        if (staticIndex === null) {
          frames = this.snapshotOperationValue(
            frames,
            "i32",
            "buffer.frames",
            prelude,
            context,
          );
          if (this.bufferRefChannelMetadata(data.buffer).kind === "dynamic") {
            channels = this.snapshotOperationValue(
              channels,
              "i32",
              "buffer.channels",
              prelude,
              context,
            );
          }
        }
        const frame = this.compileDynamicBoundedIndex(
          () => this.compileValue(data.index, context),
          frames,
          data.bounds,
          context,
          true,
        );
        const index = data.channel === null
          ? frame
          : this.module.i32.add(
            this.module.i32.mul(frame, channels()),
            this.compileDynamicBoundedIndex(
              () => this.compileValue(data.channel, context),
              channels,
              data.bounds,
              context,
              true,
            ),
          );
        const pointer = this.bufferTableValueFactory(
          write ? POINTER_GLOBALS.bufferWrites : POINTER_GLOBALS.buffers,
          descriptorIndex,
          staticIndex,
          "i32",
          context,
        );
        return this.bufferPointerWithOffset(
          pointer,
          this.module.i32.mul(
            index,
            this.module.i32.const(this.scalarSize(buffer.element)),
          ),
          write,
          context,
        );
      },
    );
  }

  bufferPointerWithOffset(pointer, byteOffset, write, context) {
    const local = this.allocateGeneratedLocal(
      context,
      "i32",
      write ? "buffer.write_pointer" : "buffer.read_pointer",
    );
    const stablePointer = () => this.module.local.get(local, binaryen.i32);
    const fallback = write
      ? this.fallbackBufferWriteAddress
      : this.fallbackBufferReadAddress;
    return this.module.block(
      null,
      [
        this.module.local.set(local, pointer()),
        this.module.i32.add(
          stablePointer(),
          this.module.select(
            this.module.i32.eq(
              stablePointer(),
              this.module.i32.const(fallback),
            ),
            this.module.i32.const(0),
            byteOffset,
          ),
        ),
      ],
      binaryen.i32,
    );
  }

  compileBufferLen(bufferRef, context) {
    this.requireBufferRef(bufferRef);
    return this.withBufferRefIndex(
      bufferRef,
      context,
      binaryen.i32,
      (index, staticIndex) => this.bufferTableValueFactory(
        POINTER_GLOBALS.bufferFrames,
        index,
        staticIndex,
        "i32",
        context,
      )(),
    );
  }

  compileBufferIsBound(bufferRef, context) {
    this.requireBufferRef(bufferRef);
    return this.withBufferRefIndex(
      bufferRef,
      context,
      binaryen.i32,
      (index, staticIndex) => this.bufferBoundFactory(
        index,
        staticIndex,
        context,
      )(),
    );
  }

  compileBufferChannels(bufferRef, context) {
    const channels = this.bufferRefChannelMetadata(bufferRef);
    if (channels.kind === "mono") return this.module.i32.const(1);
    if (channels.kind === "static") return this.module.i32.const(channels.count);
    return this.withBufferRefIndex(
      bufferRef,
      context,
      binaryen.i32,
      (index, staticIndex) => this.bufferTableValueFactory(
        POINTER_GLOBALS.bufferChannels,
        index,
        staticIndex,
        "i32",
        context,
      )(),
    );
  }

  compileBufferParamLen(parameterId, context) {
    this.bufferParamType(parameterId, context);
    return this.withBufferParamSelector(
      parameterId,
      context,
      binaryen.i32,
      (selector, staticSelector) => this.bufferParamComponentFactory(
        parameterId,
        2,
        "i32",
        selector,
        staticSelector,
        context,
      )(),
    );
  }

  compileBufferParamChannels(parameterId, context) {
    const type = this.bufferParamType(parameterId, context);
    const channels = this.bufferChannelMetadata(
      type.data.channels,
      type.data.element,
    );
    if (channels.kind === "mono") return this.module.i32.const(1);
    if (channels.kind === "static") return this.module.i32.const(channels.count);
    return this.withBufferParamSelector(
      parameterId,
      context,
      binaryen.i32,
      (selector, staticSelector) => this.bufferParamComponentFactory(
        parameterId,
        3,
        "i32",
        selector,
        staticSelector,
        context,
      )(),
    );
  }

  bufferParamType(parameterId, context) {
    const parameter = context.function.params[this.bufferParamIds(parameterId, context)[0]];
    const type = parameter && this.type(parameter.ty);
    const expectedKind = parameterId?.kind === "array_element"
      ? "buffer_span"
      : "buffer";
    if (!type || type.kind !== expectedKind) {
      this.fail(`parameter id ${parameterId} is not a buffer`);
    }
    return type;
  }

  bufferParamIds(parameterRef, context) {
    if (Number.isInteger(parameterRef)) return [parameterRef];
    if (
      parameterRef?.kind === "direct"
      && Number.isInteger(parameterRef.data)
    ) {
      return [parameterRef.data];
    }
    if (
      parameterRef?.kind === "array_element"
      && Number.isInteger(parameterRef.data?.span)
      && parameterRef.data.span >= 0
      && parameterRef.data.span < context.function.params.length
    ) {
      return [parameterRef.data.span];
    }
    this.fail("invalid buffer parameter reference");
  }

  bufferParamLayout(parameterId, context) {
    const layout = context.paramLayouts[parameterId];
    if (!layout || !["buffer", "buffer_span"].includes(layout.kind)) {
      this.fail(`parameter id ${parameterId} has no buffer descriptor`);
    }
    return layout;
  }

  staticBufferParamSelector(parameterRef, context) {
    if (parameterRef?.kind !== "array_element") return null;
    const reference = parameterRef.data;
    const type = this.bufferParamType(parameterRef, context);
    const selector = reference.selector;
    if (
      selector?.kind !== "constant"
      || selector.data?.type !== "i32"
      || !Number.isInteger(selector.data.value)
    ) {
      return null;
    }
    let index = selector.data.value;
    if (reference.bounds === "clamp") {
      index = Math.min(type.data.len - 1, Math.max(0, index));
    } else if (index < 0 || index >= type.data.len) {
      return null;
    }
    return index;
  }

  compileBufferParamSelector(parameterRef, context) {
    const reference = parameterRef.data;
    const type = this.bufferParamType(parameterRef, context);
    return this.compileDynamicBoundedIndex(
      () => this.compileValue(reference.selector, context),
      () => this.module.i32.const(type.data.len),
      reference.bounds,
      context,
      true,
    );
  }

  withBufferParamSelector(parameterRef, context, resultType, build) {
    if (parameterRef?.kind !== "array_element") {
      return build(null, null, []);
    }
    const staticSelector = this.staticBufferParamSelector(parameterRef, context);
    if (staticSelector !== null) {
      return build(
        () => this.module.i32.const(staticSelector),
        staticSelector,
        [],
      );
    }
    const selectorLocal = this.allocateGeneratedLocal(
      context,
      "i32",
      "buffer_param.selector",
    );
    const prelude = [
      this.module.local.set(
        selectorLocal,
        this.compileBufferParamSelector(parameterRef, context),
      ),
    ];
    const result = build(
      () => this.module.local.get(selectorLocal, binaryen.i32),
      null,
      prelude,
    );
    return this.module.block(null, [...prelude, result], resultType);
  }

  bufferParamComponentFactory(
    parameterRef,
    offset,
    scalar,
    selector,
    staticSelector,
    context,
  ) {
    const parameterId = this.bufferParamIds(parameterRef, context)[0];
    const layout = this.bufferParamLayout(parameterId, context);
    if (parameterRef?.kind !== "array_element") {
      return () =>
        this.module.local.get(layout.index + offset, this.wasmType(scalar));
    }
    const rawLoad = (index) => {
      const table = this.module.local.get(layout.index + offset, binaryen.i32);
      const address = this.module.i32.add(
        table,
        this.module.i32.mul(
          index,
          this.module.i32.const(this.scalarSize(scalar)),
        ),
      );
      return this.loadScalar(scalar, address);
    };
    const load = (index) => {
      if (offset === 5) {
        return this.module.i32.ne(rawLoad(index), this.module.i32.const(0));
      }
      if (scalar !== "i32" || (offset !== 0 && offset !== 1)) {
        return rawLoad(index);
      }
      return this.resolveBufferPointer(
        () => rawLoad(index),
        offset === 1,
        context,
      );
    };
    if (staticSelector === null) return () => load(selector());

    const key = `buffer_param:${parameterId}:${staticSelector}:${offset}:${scalar}`;
    let local = context.bufferDescriptorCache.get(key);
    if (local === undefined) {
      local = this.allocateGeneratedLocal(
        context,
        scalar,
        `buffer_param.component${offset}.${parameterId}.${staticSelector}`,
      );
      context.bufferDescriptorCache.set(key, local);
      context.entryInitializers.push(
        this.module.local.set(
          local,
          load(this.module.i32.const(staticSelector)),
        ),
      );
    }
    return () => this.module.local.get(local, this.wasmType(scalar));
  }

  bufferParamChannelsFactory(
    parameterRef,
    selector,
    staticSelector,
    context,
  ) {
    const type = this.bufferParamType(parameterRef, context);
    const channels = this.bufferChannelMetadata(
      type.data.channels,
      type.data.element,
    );
    if (channels.kind === "mono") return () => this.module.i32.const(1);
    if (channels.kind === "static") {
      return () => this.module.i32.const(channels.count);
    }
    return this.bufferParamComponentFactory(
      parameterRef,
      3,
      "i32",
      selector,
      staticSelector,
      context,
    );
  }

  loadBufferParamComponent(parameterId, offset, scalar, context) {
    return this.withBufferParamSelector(
      parameterId,
      context,
      this.wasmType(scalar),
      (selector, staticSelector) => this.bufferParamComponentFactory(
        parameterId,
        offset,
        scalar,
        selector,
        staticSelector,
        context,
      )(),
    );
  }

  loadBufferParamValue(parameterId, context) {
    const components = ["i32", "i32", "i32", "i32", "f32", "i32"];
    if (parameterId?.kind !== "array_element") {
      return components.map((scalar, offset) =>
        this.loadBufferParamComponent(parameterId, offset, scalar, context),
      );
    }
    const staticSelector = this.staticBufferParamSelector(parameterId, context);
    let selector;
    let initializeSelector = null;
    if (staticSelector === null) {
      const selectorLocal = this.allocateGeneratedLocal(
        context,
        "i32",
        "buffer_param.selector",
      );
      selector = () => this.module.local.get(selectorLocal, binaryen.i32);
      initializeSelector = this.module.local.set(
        selectorLocal,
        this.compileBufferParamSelector(parameterId, context),
      );
    } else {
      selector = () => this.module.i32.const(staticSelector);
    }
    const values = components.map((scalar, offset) =>
      this.bufferParamComponentFactory(
        parameterId,
        offset,
        scalar,
        selector,
        staticSelector,
        context,
      )(),
    );
    if (initializeSelector !== null) {
      values[0] = this.module.block(
        null,
        [initializeSelector, values[0]],
        binaryen.i32,
      );
    }
    return values;
  }

  loadBufferPlace(place, context) {
    if (place.base.kind !== "parameter" || place.projections.length !== 0) {
      this.fail("buffer call arguments must be unprojected buffer parameters");
    }
    const layout = this.bufferParamLayout(place.base.data, context);
    return layout.components.map((scalar, offset) =>
      this.module.local.get(layout.index + offset, this.wasmType(scalar)),
    );
  }

  compileInterfaceBufferValue(bufferRef, context) {
    this.requireBufferRef(bufferRef);
    const staticIndex = this.staticBufferRefIndex(bufferRef);
    let descriptorIndex;
    let initializeIndex = null;
    if (staticIndex === null) {
      const indexLocal = this.allocateGeneratedLocal(
        context,
        "i32",
        "buffer.descriptor_index",
      );
      descriptorIndex = () =>
        this.module.local.get(indexLocal, binaryen.i32);
      initializeIndex = this.module.local.set(
        indexLocal,
        this.compileBufferRefIndex(bufferRef, context),
      );
    } else {
      descriptorIndex = () => this.module.i32.const(staticIndex);
    }
    const component = (globalName, scalar) => this.bufferTableValueFactory(
      globalName,
      descriptorIndex,
      staticIndex,
      scalar,
      context,
    )();
    const channels = this.bufferRefChannelMetadata(bufferRef);
    const values = [
      component(POINTER_GLOBALS.buffers, "i32"),
      component(POINTER_GLOBALS.bufferWrites, "i32"),
      component(POINTER_GLOBALS.bufferFrames, "i32"),
      channels.kind === "mono"
        ? this.module.i32.const(1)
        : channels.kind === "static"
          ? this.module.i32.const(channels.count)
          : component(POINTER_GLOBALS.bufferChannels, "i32"),
      component(POINTER_GLOBALS.bufferSampleRates, "f32"),
      this.bufferBoundFactory(descriptorIndex, staticIndex, context)(),
    ];
    if (initializeIndex !== null) {
      values[0] = this.module.block(
        null,
        [initializeIndex, values[0]],
        binaryen.i32,
      );
    }
    return values;
  }

  compileBufferSpanValue(spanRef, expectedType, context) {
    const data = spanRef?.data;
    if (!data || data.len !== expectedType.data.len) {
      this.fail("buffer span argument length does not match parameter type");
    }
    const tableGlobals = [
      POINTER_GLOBALS.buffers,
      POINTER_GLOBALS.bufferWrites,
      POINTER_GLOBALS.bufferFrames,
      POINTER_GLOBALS.bufferChannels,
      POINTER_GLOBALS.bufferSampleRates,
      POINTER_GLOBALS.buffers,
    ];
    const tableScalars = ["i32", "i32", "i32", "i32", "f32", "i32"];
    let tables;
    let start;
    if (spanRef.kind === "interface") {
      if (
        !Number.isInteger(data.first)
        || data.first < 0
        || data.first + data.len > this.mir.interface.buffers.length
      ) {
        this.fail("interface buffer span is out of range");
      }
      tables = tableGlobals.map((name) => this.module.global.get(name, binaryen.i32));
      start = data.first;
    } else if (spanRef.kind === "parameter") {
      const parameter = context.function.params[data.span];
      const sourceType = parameter && this.type(parameter.ty);
      const layout = context.paramLayouts[data.span];
      if (
        !sourceType
        || sourceType.kind !== "buffer_span"
        || !layout
        || layout.kind !== "buffer_span"
        || !Number.isInteger(data.start)
        || data.start < 0
        || data.start + data.len > sourceType.data.len
      ) {
        this.fail("buffer span parameter window is out of range");
      }
      tables = layout.components.map((_, offset) =>
        this.module.local.get(layout.index + offset, binaryen.i32));
      start = data.start;
    } else {
      this.fail("invalid buffer span reference");
    }
    return tables.map((table, offset) => this.module.i32.add(
      table,
      this.module.i32.const(start * this.scalarSize(tableScalars[offset])),
    ));
  }

  loadBufferTableValue(globalName, bufferRef, scalar, context) {
    this.requireBufferRef(bufferRef);
    return this.withBufferRefIndex(
      bufferRef,
      context,
      this.wasmType(scalar),
      (index, staticIndex) => this.bufferTableValueFactory(
        globalName,
        index,
        staticIndex,
        scalar,
        context,
      )(),
    );
  }

  bufferTableValueFactory(
    globalName,
    descriptorIndex,
    staticIndex,
    scalar,
    context,
  ) {
    const load = () => {
      const raw = () => this.loadBufferTableValueAt(
        globalName,
        descriptorIndex(),
        scalar,
      );
      if (
        scalar === "i32"
        && (globalName === POINTER_GLOBALS.buffers
          || globalName === POINTER_GLOBALS.bufferWrites)
      ) {
        return this.resolveBufferPointer(
          raw,
          globalName === POINTER_GLOBALS.bufferWrites,
          context,
        );
      }
      return raw();
    };
    if (staticIndex === null) {
      return load;
    }
    const key = `${globalName}:${staticIndex}:${scalar}`;
    let local = context.bufferDescriptorCache.get(key);
    if (local === undefined) {
      local = this.allocateGeneratedLocal(
        context,
        scalar,
        `buffer.${globalName.slice(6)}.${staticIndex}`,
      );
      context.bufferDescriptorCache.set(key, local);
      context.entryInitializers.push(
        this.module.local.set(
          local,
          load(),
        ),
      );
    }
    return () => this.module.local.get(local, this.wasmType(scalar));
  }

  bufferBoundFactory(descriptorIndex, staticIndex, context) {
    const load = () => this.module.i32.ne(
      this.loadBufferTableValueAt(
        POINTER_GLOBALS.buffers,
        descriptorIndex(),
        "i32",
      ),
      this.module.i32.const(0),
    );
    if (staticIndex === null) return load;

    const key = `buffer_bound:${staticIndex}`;
    let local = context.bufferDescriptorCache.get(key);
    if (local === undefined) {
      local = this.allocateGeneratedLocal(
        context,
        "i32",
        `buffer.bound.${staticIndex}`,
      );
      context.bufferDescriptorCache.set(key, local);
      context.entryInitializers.push(this.module.local.set(local, load()));
    }
    return () => this.module.local.get(local, binaryen.i32);
  }

  bufferChannelsFactory(
    bufferRef,
    descriptorIndex,
    staticIndex,
    context,
  ) {
    const channels = this.bufferRefChannelMetadata(bufferRef);
    if (channels.kind === "mono") {
      return () => this.module.i32.const(1);
    }
    if (channels.kind === "static") {
      return () => this.module.i32.const(channels.count);
    }
    return this.bufferTableValueFactory(
      POINTER_GLOBALS.bufferChannels,
      descriptorIndex,
      staticIndex,
      "i32",
      context,
    );
  }

  loadBufferTableValueAt(globalName, descriptorIndex, scalar) {
    const size = this.scalarSize(scalar);
    const load = () => this.loadScalar(
      scalar,
      this.module.i32.add(
        this.module.global.get(globalName, binaryen.i32),
        this.module.i32.mul(
          descriptorIndex,
          this.module.i32.const(size),
        ),
      ),
    );
    return load();
  }

  resolveBufferPointer(load, write, context) {
    const local = this.allocateGeneratedLocal(
      context,
      "i32",
      write ? "buffer.write_or_discard" : "buffer.read_or_zero",
    );
    const pointer = () => this.module.local.get(local, binaryen.i32);
    return this.module.block(
      null,
      [
        this.module.local.set(local, load()),
        this.module.select(
          this.module.i32.ne(pointer(), this.module.i32.const(0)),
          pointer(),
          this.module.i32.const(
            write
              ? this.fallbackBufferWriteAddress
              : this.fallbackBufferReadAddress,
          ),
        ),
      ],
      binaryen.i32,
    );
  }

  withBufferRefIndex(bufferRef, context, resultType, build) {
    const staticIndex = this.staticBufferRefIndex(bufferRef);
    if (staticIndex !== null) {
      return build(
        () => this.module.i32.const(staticIndex),
        staticIndex,
        [],
      );
    }
    const indexLocal = this.allocateGeneratedLocal(
      context,
      "i32",
      "buffer.descriptor_index",
    );
    const prelude = [
      this.module.local.set(
        indexLocal,
        this.compileBufferRefIndex(bufferRef, context),
      ),
    ];
    const result = build(
      () => this.module.local.get(indexLocal, binaryen.i32),
      null,
      prelude,
    );
    return this.module.block(null, [...prelude, result], resultType);
  }

  snapshotOperationValue(factory, scalar, name, prelude, context) {
    const local = this.allocateGeneratedLocal(context, scalar, name);
    prelude.push(
      this.module.local.set(local, factory()),
    );
    return () => this.module.local.get(local, this.wasmType(scalar));
  }

  allocateGeneratedLocal(context, scalar, name) {
    const index = context.generatedLocalBase + context.generatedLocals.length;
    context.generatedLocals.push({
      index,
      scalar,
      name: `${name}.generated${context.generatedLocals.length}`,
    });
    return index;
  }

  compileSliceValue(value, context) {
    if (value.kind !== "local") {
      this.fail("slice values must reside in MIR locals");
    }
    const layout = context.localLayouts[value.data];
    if (!layout || layout.kind !== "slice") {
      this.fail(`local id ${value.data} is not a slice`);
    }
    return layout.components.map((scalar, offset) =>
      this.module.local.get(layout.index + offset, this.wasmType(scalar)),
    );
  }

  loadSlicePlace(place, context) {
    if (place.projections.length !== 0) {
      this.fail("slice places cannot have projections");
    }
    let layout;
    if (place.base.kind === "local") {
      layout = context.localLayouts[place.base.data];
    } else if (place.base.kind === "parameter") {
      layout = context.paramLayouts[place.base.data];
    } else if (place.base.kind === "event_param") {
      const event = this.mir.interface.events[context.eventId];
      const parameter = event?.params[place.base.data];
      const type = parameter && this.type(parameter.ty);
      if (!type || type.kind !== "slice") {
        this.fail(`event parameter id ${place.base.data} is not a slice`);
      }
      const header = () =>
        this.compileEventParamAddress(context.eventId, place.base.data);
      const address = () =>
        this.module.i32.add(header(), this.module.i32.const(4));
      return [
        address(),
        address(),
        this.module.i32.load(0, 4, header()),
        this.module.i32.const(this.scalarSize(type.data.element)),
      ];
    } else {
      this.fail(`slice place base '${place.base.kind}' is not supported yet`);
    }
    if (!layout || layout.kind !== "slice") {
      this.fail(`place base '${place.base.kind}' id ${place.base.data} is not a slice`);
    }
    return layout.components.map((scalar, offset) =>
      this.module.local.get(layout.index + offset, this.wasmType(scalar)),
    );
  }

  compileEventParamAddress(eventId, paramId) {
    const event = this.mir.interface.events[eventId];
    if (!event || !Number.isInteger(paramId) || paramId < 0 || paramId >= event.params.length) {
      this.fail(`event parameter id ${paramId} is invalid for event ${eventId}`);
    }
    let offset = () => this.module.i32.const(0);
    for (let index = 0; index < paramId; index += 1) {
      const previous = offset;
      const type = this.type(event.params[index].ty);
      if (type.kind === "slice") {
        const elementSize = this.scalarSize(type.data.element);
        offset = () =>
          this.module.i32.add(
            previous(),
            this.module.i32.add(
              this.module.i32.const(4),
              this.module.i32.mul(
                this.module.i32.load(
                  0,
                  4,
                  this.module.i32.add(
                    this.module.global.get(POINTER_GLOBALS.eventPayload, binaryen.i32),
                    previous(),
                  ),
                ),
                this.module.i32.const(elementSize),
              ),
            ),
          );
      } else {
        const size = this.typeLayout(event.params[index].ty).size;
        offset = () => this.module.i32.add(previous(), this.module.i32.const(size));
      }
    }
    return this.module.i32.add(
      this.module.global.get(POINTER_GLOBALS.eventPayload, binaryen.i32),
      offset(),
    );
  }

  storeSlicePlace(place, components, context) {
    if (place.base.kind !== "local" || place.projections.length !== 0) {
      this.fail("slice assignment destination must be an unprojected local");
    }
    const layout = context.localLayouts[place.base.data];
    if (!layout || layout.kind !== "slice" || components.length !== 4) {
      this.fail(`local id ${place.base.data} is not a valid slice destination`);
    }
    return this.module.block(
      null,
      components.map((component, offset) =>
        this.module.local.set(layout.index + offset, component),
      ),
    );
  }

  compileMakeSlice(data, context) {
    const sourceValues = this.compileSliceSource(data.source, context);
    const sourceLocals = sourceValues.map((_, offset) =>
      this.allocateGeneratedLocal(
        context,
        "i32",
        `slice.source_component${offset}`,
      ),
    );
    const initializeSource = sourceValues.map((value, offset) =>
      this.module.local.set(sourceLocals[offset], value),
    );
    const source = (offset) => () =>
      this.module.local.get(sourceLocals[offset], binaryen.i32);
    const range = this.compileSliceRange(
      () => this.compileValue(data.start, context),
      () => this.compileValue(data.len, context),
      source(2),
      data.bounds,
      context,
    );
    const result = [
      this.module.i32.add(
        source(0)(),
        this.module.i32.mul(
          range.start(),
          source(3)(),
        ),
      ),
      this.module.i32.add(
        source(1)(),
        this.module.i32.mul(
          range.start(),
          source(3)(),
        ),
      ),
      range.len(),
      source(3)(),
    ];
    result[0] = this.module.block(
      null,
      [...initializeSource, result[0]],
      binaryen.i32,
    );
    return result;
  }

  compileSliceRange(start, len, sourceLen, bounds, context) {
    const zero = () => this.module.i32.const(0);
    if (bounds === "unchecked") {
      return { start, len };
    }
    if (bounds === "clamp") {
      const normalizedStart = () => {
        const low = () =>
          this.module.select(
            this.module.i32.lt_s(start(), zero()),
            zero(),
            start(),
          );
        return this.module.select(
          this.module.i32.gt_s(low(), sourceLen()),
          sourceLen(),
          low(),
        );
      };
      const normalizedLen = () => {
        const low = () =>
          this.module.select(
            this.module.i32.lt_s(len(), zero()),
            zero(),
            len(),
          );
        const remaining = () =>
          this.module.i32.sub(sourceLen(), normalizedStart());
        return this.module.select(
          this.module.i32.gt_s(low(), remaining()),
          remaining(),
          low(),
        );
      };
      return { start: normalizedStart, len: normalizedLen };
    }
    if (bounds === "checked") {
      const invalid = () => {
        const remaining = () => this.module.i32.sub(sourceLen(), start());
        return this.module.i32.or(
          this.module.i32.or(
            this.module.i32.lt_s(start(), zero()),
            this.module.i32.gt_s(start(), sourceLen()),
          ),
          this.module.i32.or(
            this.module.i32.lt_s(len(), zero()),
            this.module.i32.gt_s(len(), remaining()),
          ),
        );
      };
      return {
        start: () =>
          this.module.if(invalid(), this.raiseRuntimeFailure(context), start()),
        len,
      };
    }
    this.fail(`unknown bounds mode '${String(bounds)}'`);
  }
}
