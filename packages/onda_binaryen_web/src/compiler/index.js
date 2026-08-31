import binaryen from "binaryen";
import { MirCompilerLowering } from "./lowering.js";
import {
  OndaBinaryenError,
  supportsMirOperation,
  PROCESSOR_ABI_VERSION,
  PROCESSOR_ARTIFACT_FORMAT,
  PROCESSOR_ARTIFACT_FORMAT_VERSION,
  PROCESSOR_SNAPSHOT_FORMAT_VERSION,
  WASM32_ADDRESS_SPACE_BYTES,
  DELEGATE_RECORD_HEADER_SIZE,
  PRINT_RECORD_HEADER_SIZE,
  POINTER_GLOBALS,
  typeName,
  decodeI64Literal,
  decodeFloatLiteral,
} from "./shared.js";

export class MirCompiler extends MirCompilerLowering {
  compileSliceSource(source, context) {
    if (source.kind === "place") {
      const typeId = this.placeTypeId(source.data, context);
      const type = this.type(typeId);
      if (type.kind === "slice") {
        return this.loadSlicePlace(source.data, context);
      }
      if (type.kind !== "array") {
        this.fail(`slice place source has unsupported type '${type.kind}'`);
      }
      const element = this.type(type.data.element);
      if (element.kind !== "scalar") {
        this.fail("slice array source must have primitive elements");
      }
      return [
        this.placeAddress(source.data, context),
        this.placeAddress(source.data, context),
        this.module.i32.const(type.data.len),
        this.module.i32.const(this.scalarSize(element.data)),
      ];
    }
    if (source.kind === "const_data") {
      const item = this.constLayout[source.data];
      if (!item) this.fail(`const data id ${source.data} is out of range`);
      return [
        this.module.i32.const(item.address),
        this.module.i32.const(item.address),
        this.module.i32.const(item.len),
        this.module.i32.const(this.scalarSize(item.scalar)),
      ];
    }
    if (source.kind === "buffer") {
      const buffer = this.requireBufferRef(source.data.buffer);
      const elementSize = this.scalarSize(buffer.element);
      const staticIndex = this.staticBufferRefIndex(source.data.buffer);
      const prelude = [];
      let descriptorIndex;
      if (staticIndex === null) {
        const indexLocal = this.allocateGeneratedLocal(
          context,
          "i32",
          "buffer.descriptor_index",
        );
        prelude.push(
          this.module.local.set(
            indexLocal,
            this.compileBufferRefIndex(source.data.buffer, context),
          ),
        );
        descriptorIndex = () =>
          this.module.local.get(indexLocal, binaryen.i32);
      } else {
        descriptorIndex = () => this.module.i32.const(staticIndex);
      }
      const component = (globalName, scalar) => this.bufferTableValueFactory(
        globalName,
        descriptorIndex,
        staticIndex,
        scalar,
        context,
      );
      let channels = this.bufferChannelsFactory(
        source.data.buffer,
        descriptorIndex,
        staticIndex,
        context,
      );
      if (
        staticIndex === null
        && this.bufferRefChannelMetadata(source.data.buffer).kind === "dynamic"
      ) {
        channels = this.snapshotOperationValue(
          channels,
          "i32",
          "buffer.channels",
          prelude,
          context,
        );
      }
      const readAddress = component(POINTER_GLOBALS.buffers, "i32");
      const writeAddress = component(POINTER_GLOBALS.bufferWrites, "i32");
      let read = readAddress();
      let write = writeAddress();
      if (source.data.channel !== null) {
        const channelLocal = this.allocateGeneratedLocal(
          context,
          "i32",
          "buffer.slice_channel",
        );
        prelude.push(
          this.module.local.set(
            channelLocal,
            this.compileDynamicBoundedIndex(
              () => this.compileValue(source.data.channel, context),
              channels,
              "clamp",
              context,
              true,
            ),
          ),
        );
        const channelOffset = () => this.module.i32.mul(
          this.module.local.get(channelLocal, binaryen.i32),
          this.module.i32.const(elementSize),
        );
        read = this.bufferPointerWithOffset(
          readAddress,
          channelOffset(),
          false,
          context,
        );
        write = this.bufferPointerWithOffset(
          writeAddress,
          channelOffset(),
          true,
          context,
        );
      }
      const result = [
        read,
        write,
        component(POINTER_GLOBALS.bufferFrames, "i32")(),
        source.data.channel === null
          ? this.module.i32.const(elementSize)
          : this.module.i32.mul(channels(), this.module.i32.const(elementSize)),
      ];
      if (prelude.length > 0) {
        result[0] = this.module.block(
          null,
          [...prelude, result[0]],
          binaryen.i32,
        );
      }
      return result;
    }
    if (source.kind === "buffer_param") {
      const type = this.bufferParamType(source.data.parameter, context);
      const elementSize = this.scalarSize(type.data.element);
      const staticSelector = this.staticBufferParamSelector(
        source.data.parameter,
        context,
      );
      const prelude = [];
      let selector = null;
      if (
        source.data.parameter?.kind === "array_element"
        && staticSelector === null
      ) {
        const selectorLocal = this.allocateGeneratedLocal(
          context,
          "i32",
          "buffer_param.selector",
        );
        prelude.push(
          this.module.local.set(
            selectorLocal,
            this.compileBufferParamSelector(source.data.parameter, context),
          ),
        );
        selector = () => this.module.local.get(selectorLocal, binaryen.i32);
      } else if (staticSelector !== null) {
        selector = () => this.module.i32.const(staticSelector);
      }
      const component = (offset, scalar) => this.bufferParamComponentFactory(
        source.data.parameter,
        offset,
        scalar,
        selector,
        staticSelector,
        context,
      );
      let channels = this.bufferParamChannelsFactory(
        source.data.parameter,
        selector,
        staticSelector,
        context,
      );
      if (
        source.data.parameter?.kind === "array_element"
        && staticSelector === null
        && this.bufferChannelMetadata(
          type.data.channels,
          type.data.element,
        ).kind === "dynamic"
      ) {
        channels = this.snapshotOperationValue(
          channels,
          "i32",
          "buffer_param.channels",
          prelude,
          context,
        );
      }
      const readAddress = component(0, "i32");
      const writeAddress = component(1, "i32");
      let read = readAddress();
      let write = writeAddress();
      if (source.data.channel !== null) {
        const channelLocal = this.allocateGeneratedLocal(
          context,
          "i32",
          "buffer_param.slice_channel",
        );
        prelude.push(
          this.module.local.set(
            channelLocal,
            this.compileDynamicBoundedIndex(
              () => this.compileValue(source.data.channel, context),
              channels,
              "clamp",
              context,
              true,
            ),
          ),
        );
        const channelOffset = () => this.module.i32.mul(
          this.module.local.get(channelLocal, binaryen.i32),
          this.module.i32.const(elementSize),
        );
        read = this.bufferPointerWithOffset(
          readAddress,
          channelOffset(),
          false,
          context,
        );
        write = this.bufferPointerWithOffset(
          writeAddress,
          channelOffset(),
          true,
          context,
        );
      }
      const result = [
        read,
        write,
        component(2, "i32")(),
        source.data.channel === null
          ? this.module.i32.const(elementSize)
          : this.module.i32.mul(channels(), this.module.i32.const(elementSize)),
      ];
      if (prelude.length > 0) {
        result[0] = this.module.block(
          null,
          [...prelude, result[0]],
          binaryen.i32,
        );
      }
      return result;
    }
    this.fail(`unsupported slice source '${String(source.kind)}'`);
  }

  sliceElementScalar(value, context) {
    if (value.kind !== "local") this.fail("slice value is not a local");
    const local = context.function.locals[value.data];
    const type = local && this.type(local.ty);
    if (!type || type.kind !== "slice") {
      this.fail(`local id ${value.data} is not slice-typed`);
    }
    return type.data.element;
  }

  sliceAccess(value, context) {
    if (value.kind !== "local") this.fail("slice value is not a local");
    const local = context.function.locals[value.data];
    const type = local && this.type(local.ty);
    if (!type || type.kind !== "slice") {
      this.fail(`local id ${value.data} is not slice-typed`);
    }
    return type.data.access;
  }

  compileSliceAddress(slice, index, bounds, context, write) {
    return this.compileSliceAddressWithFactories(
      () => this.compileSliceValue(slice, context),
      () => this.compileValue(index, context),
      bounds,
      context,
      write,
    );
  }

  compileSliceAddressWithFactories(slice, index, bounds, context, write) {
    const bounded = this.compileDynamicBoundedIndex(
      index,
      () => slice()[2],
      bounds,
      context,
    );
    return this.module.i32.add(
      slice()[write ? 1 : 0],
      this.module.i32.mul(bounded, slice()[3]),
    );
  }

  compileArrayWindowAddress(data, parameterType, context) {
    const sourceTypeId = this.placeTypeId(data.array, context);
    const sourceType = this.type(sourceTypeId);
    if (sourceType.kind !== "array") {
      this.fail("array-window source is not a fixed array");
    }
    if (
      !this.typesEquivalent(sourceType.data.element, parameterType.data.element) ||
      sourceType.data.len < parameterType.data.len
    ) {
      this.fail("array-window source does not contain the required parameter shape");
    }
    const elementSize = this.typeLayout(parameterType.data.element).size;
    const start = this.compileWindowStart(
      () => this.compileValue(data.start, context),
      () =>
        this.module.i32.const(
          sourceType.data.len - parameterType.data.len,
        ),
      data.bounds,
      context,
    );
    return this.module.i32.add(
      this.placeAddress(data.array, context),
      this.module.i32.mul(start(), this.module.i32.const(elementSize)),
    );
  }

  compileSliceWindowAddress(data, parameterType, context, write) {
    const elementType = this.type(parameterType.data.element);
    if (elementType.kind !== "scalar") {
      this.fail("slice-window fixed-array parameter element is not scalar");
    }
    const slice = () => this.compileSliceValue(data.slice, context);
    const elementSize = this.scalarSize(elementType.data);
    const requiredLen = parameterType.data.len;
    const start = this.compileWindowStart(
      () => this.compileValue(data.start, context),
      () =>
        this.module.i32.sub(
          slice()[2],
          this.module.i32.const(requiredLen),
        ),
      data.bounds,
      context,
    );
    const address = () =>
      this.module.i32.add(
        slice()[write ? 1 : 0],
        this.module.i32.mul(start(), slice()[3]),
      );
    if (data.bounds === "unchecked") {
      return address();
    }
    const invalidShape = () =>
      this.module.i32.or(
        this.module.i32.ne(
          slice()[3],
          this.module.i32.const(elementSize),
        ),
        this.module.i32.lt_s(
          slice()[2],
          this.module.i32.const(requiredLen),
        ),
      );
    return this.module.if(
      invalidShape(),
      this.raiseRuntimeFailure(context),
      address(),
    );
  }

  compileWindowStart(start, maximum, bounds, context) {
    const zero = () => this.module.i32.const(0);
    if (bounds === "unchecked") {
      return start;
    }
    if (bounds === "clamp") {
      return () => {
        const low = () =>
          this.module.select(
            this.module.i32.lt_s(start(), zero()),
            zero(),
            start(),
          );
        return this.module.select(
          this.module.i32.gt_s(low(), maximum()),
          maximum(),
          low(),
        );
      };
    }
    if (bounds === "checked") {
      return () =>
        this.module.if(
          this.module.i32.or(
            this.module.i32.lt_s(start(), zero()),
            this.module.i32.gt_s(start(), maximum()),
          ),
          this.raiseRuntimeFailure(context),
          start(),
        );
    }
    this.fail(`unknown bounds mode '${String(bounds)}'`);
  }

  compileSliceLoad(data, context) {
    const scalar = this.sliceElementScalar(data.slice, context);
    return this.loadScalar(
      scalar,
      this.compileSliceAddress(data.slice, data.index, data.bounds, context, false),
    );
  }

  compileSliceStore(data, context) {
    if (this.sliceAccess(data.slice, context) !== "read_write") {
      this.fail("slice store destination is read-only");
    }
    const scalar = this.sliceElementScalar(data.slice, context);
    return this.storeScalar(
      scalar,
      this.compileSliceAddress(data.slice, data.index, data.bounds, context, true),
      this.compileValue(data.value, context),
    );
  }

  compileSliceFill(statement, data, context) {
    if (this.sliceAccess(data.destination, context) !== "read_write") {
      this.fail("slice fill destination is read-only");
    }
    const scratch = context.sliceScratch.get(statement);
    if (!scratch || scratch.count !== 1) {
      this.fail("internal slice-fill scratch local is missing");
    }
    const counter = scratch.index;
    const id = this.nextLabel++;
    const vectorLoopLabel = `$onda.slice.fill.vector.${id}`;
    const scalarLoopLabel = `$onda.slice.fill.scalar.${id}`;
    const destination = () => this.compileSliceValue(data.destination, context);
    const counterValue = () => this.module.local.get(counter, binaryen.i32);
    const scalar = this.sliceElementScalar(data.destination, context);
    const scalarSize = this.scalarSize(scalar);
    const address = () =>
      this.module.i32.add(
        destination()[1],
        this.module.i32.mul(counterValue(), destination()[3]),
      );
    const scalarLoop = () =>
      this.module.loop(
        scalarLoopLabel,
        this.module.if(
          this.module.i32.lt_s(counterValue(), destination()[2]),
          this.module.block(null, [
            this.storeScalar(
              scalar,
              address(),
              this.compileValue(data.value, context),
            ),
            this.module.local.set(
              counter,
              this.module.i32.add(counterValue(), this.module.i32.const(1)),
            ),
            this.module.br(scalarLoopLabel),
          ]),
        ),
      );
    const body = [];
    if (this.options.simd) {
      const lanes = 16 / scalarSize;
      const vectorCondition = this.module.i32.and(
        this.module.i32.eq(
          destination()[3],
          this.module.i32.const(scalarSize),
        ),
        this.module.i32.and(
          this.module.i32.ge_u(
            destination()[2],
            this.module.i32.const(lanes),
          ),
          this.module.i32.le_u(
            counterValue(),
            this.module.i32.sub(
              destination()[2],
              this.module.i32.const(lanes),
            ),
          ),
        ),
      );
      body.push(
        this.module.loop(
          vectorLoopLabel,
          this.module.if(
            vectorCondition,
            this.module.block(null, [
              this.module.v128.store(
                0,
                scalarSize,
                address(),
                this.compileVectorSplat(
                  scalar,
                  this.compileValue(data.value, context),
                ),
              ),
              this.module.local.set(
                counter,
                this.module.i32.add(
                  counterValue(),
                  this.module.i32.const(lanes),
                ),
              ),
              this.module.br(vectorLoopLabel),
            ]),
          ),
        ),
      );
    }
    body.push(scalarLoop());
    return this.module.block(null, [
      this.module.local.set(counter, this.module.i32.const(0)),
      ...body,
    ]);
  }

  compileSliceCopy(statement, data, context) {
    if (this.sliceAccess(data.destination, context) !== "read_write") {
      this.fail("slice copy destination is read-only");
    }
    const scratch = context.sliceScratch.get(statement);
    if (!scratch || scratch.count !== 2) {
      this.fail("internal slice-copy scratch locals are missing");
    }
    const count = scratch.index;
    const counter = scratch.index + 1;
    const id = this.nextLabel++;
    const loopLabel = `$onda.slice.copy.${id}`;
    const destination = () => this.compileSliceValue(data.destination, context);
    const source = () => this.compileSliceValue(data.source, context);
    const countValue = () => this.module.local.get(count, binaryen.i32);
    const counterValue = () => this.module.local.get(counter, binaryen.i32);
    const copyIndex = () =>
      this.module.select(
        this.module.i32.and(
          this.module.i32.eq(destination()[3], source()[3]),
          this.module.i32.gt_u(destination()[1], source()[0]),
        ),
        this.module.i32.sub(
          this.module.i32.sub(countValue(), this.module.i32.const(1)),
          counterValue(),
        ),
        counterValue(),
      );
    const sourceAddress = () =>
      this.module.i32.add(
        source()[0],
        this.module.i32.mul(copyIndex(), source()[3]),
      );
    const destinationAddress = () =>
      this.module.i32.add(
        destination()[1],
        this.module.i32.mul(copyIndex(), destination()[3]),
      );
    const sourceScalar = this.sliceElementScalar(data.source, context);
    const destinationScalar = this.sliceElementScalar(data.destination, context);
    const copiedValue = () => {
      const loaded = this.loadScalar(sourceScalar, sourceAddress());
      return sourceScalar === destinationScalar
        ? loaded
        : this.compileCast(sourceScalar, destinationScalar, loaded);
    };
    const nonEmpty = () =>
      this.module.i32.gt_s(countValue(), this.module.i32.const(0));
    const sourceEnd = () =>
      this.module.i32.add(
        this.module.i32.add(
          source()[0],
          this.module.i32.mul(
            this.module.i32.sub(countValue(), this.module.i32.const(1)),
            source()[3],
          ),
        ),
        this.module.i32.const(this.scalarSize(sourceScalar)),
      );
    const destinationEnd = () =>
      this.module.i32.add(
        this.module.i32.add(
          destination()[1],
          this.module.i32.mul(
            this.module.i32.sub(countValue(), this.module.i32.const(1)),
            destination()[3],
          ),
        ),
        this.module.i32.const(this.scalarSize(destinationScalar)),
      );
    const overlaps = () =>
      this.module.i32.and(
        nonEmpty(),
        this.module.i32.and(
          this.module.i32.lt_u(destination()[1], sourceEnd()),
          this.module.i32.lt_u(source()[0], destinationEnd()),
        ),
      );
    const invalidOverlap = () =>
      this.module.i32.and(
        this.module.i32.ne(destination()[3], source()[3]),
        overlaps(),
      );
    const scalarCopy = () =>
      this.module.loop(
        loopLabel,
        this.module.if(
          this.module.i32.lt_s(counterValue(), countValue()),
          this.module.block(null, [
            this.storeScalar(destinationScalar, destinationAddress(), copiedValue()),
            this.module.local.set(
              counter,
              this.module.i32.add(counterValue(), this.module.i32.const(1)),
            ),
            this.module.br(loopLabel),
          ]),
        ),
      );
    const sameRepresentation = sourceScalar === destinationScalar;
    const copy = sameRepresentation
      ? this.module.if(
          this.module.i32.and(
            this.module.i32.eq(
              destination()[3],
              this.module.i32.const(this.scalarSize(destinationScalar)),
            ),
            this.module.i32.eq(
              source()[3],
              this.module.i32.const(this.scalarSize(sourceScalar)),
            ),
          ),
          // memory.copy has memmove overlap semantics and lets engines use
          // their tuned bulk-memory implementation for contiguous slices.
          this.module.memory.copy(
            destination()[1],
            source()[0],
            this.module.i32.mul(
              countValue(),
              this.module.i32.const(this.scalarSize(sourceScalar)),
            ),
          ),
          scalarCopy(),
        )
      : scalarCopy();
    return this.module.block(null, [
      this.module.local.set(
        count,
        this.module.select(
          this.module.i32.lt_s(destination()[2], source()[2]),
          destination()[2],
          source()[2],
        ),
      ),
      this.module.if(invalidOverlap(), this.raiseRuntimeFailure(context)),
      this.module.local.set(counter, this.module.i32.const(0)),
      copy,
    ]);
  }

  compileVectorSplat(scalar, value) {
    switch (scalar) {
      case "bool": return this.module.i8x16.splat(value);
      case "i32": return this.module.i32x4.splat(value);
      case "i64": return this.module.i64x2.splat(value);
      case "f32": return this.module.f32x4.splat(value);
      case "f64": return this.module.f64x2.splat(value);
      default: this.fail(`unknown SIMD scalar type '${String(scalar)}'`);
    }
  }

  compileValue(value, context) {
    switch (value.kind) {
      case "local": {
        const scalar = context.localScalars[value.data];
        if (!scalar) {
          this.fail(`local id ${value.data} is not a scalar or is out of range`);
        }
        return this.module.local.get(
          this.localIndex(value.data, context),
          this.wasmType(scalar),
        );
      }
      case "constant":
        return this.compileConstant(value.data);
      default:
        this.fail(`unknown MIR value '${String(value.kind)}'`);
    }
  }

  compileConstant(value) {
    switch (value.type) {
      case "f32":
        return this.module.f32.const(decodeFloatLiteral(value.value, "f32", this));
      case "f64":
        return this.module.f64.const(decodeFloatLiteral(value.value, "f64", this));
      case "i32":
        return this.module.i32.const(value.value);
      case "i64":
        return this.module.i64.const(decodeI64Literal(value.value, this));
      case "bool":
        return this.module.i32.const(value.value ? 1 : 0);
      default:
        this.fail(`unknown scalar constant type '${String(value.type)}'`);
    }
  }

  valueScalarType(value, context) {
    if (value.kind === "constant") {
      return value.data.type;
    }
    if (value.kind === "local") {
      const scalar = context.localScalars[value.data];
      if (!scalar) {
        this.fail(`local id ${value.data} is not a scalar or is out of range`);
      }
      return scalar;
    }
    this.fail(`unknown MIR value '${String(value.kind)}'`);
  }

  placeScalarType(place, context) {
    const typeId = this.placeTypeId(place, context);
    return this.requireScalarType(typeId, "place");
  }

  placeTypeId(place, context) {
    let typeId;
    switch (place.base.kind) {
      case "local":
        typeId = context.function.locals[place.base.data]?.ty;
        break;
      case "parameter":
        typeId = context.function.params[place.base.data]?.ty;
        break;
      case "state":
        typeId = this.mir.state[place.base.data]?.ty;
        break;
      case "param":
        typeId = this.mir.interface.params[place.base.data]?.ty;
        break;
      case "event_param": {
        const event = this.mir.interface.events[context.eventId];
        typeId = event?.params[place.base.data]?.ty;
        break;
      }
      default:
        this.fail(`place base '${place.base.kind}' is not supported yet`);
    }
    if (!Number.isInteger(typeId)) {
      this.fail(`place base '${place.base.kind}' id ${place.base.data} is out of range`);
    }
    for (const projection of place.projections) {
      const type = this.type(typeId);
      if (projection.kind === "index" && type.kind === "array") {
        typeId = type.data.element;
      } else {
        this.fail(`projection '${projection.kind}' on '${type.kind}' is not supported yet`);
      }
    }
    return typeId;
  }

  loadPlace(place, context) {
    if (place.base.kind === "local" && place.projections.length === 0) {
      const scalar = this.placeScalarType(place, context);
      return this.module.local.get(
        this.localIndex(place.base.data, context),
        this.wasmType(scalar),
      );
    }
    if (place.base.kind === "parameter" && place.projections.length === 0) {
      const scalar = this.placeScalarType(place, context);
      const layout = context.paramLayouts[place.base.data];
      if (!layout || !["scalar", "scalar_ref"].includes(layout.kind)) {
        this.fail(`parameter id ${place.base.data} is not a scalar`);
      }
      if (layout.kind === "scalar_ref") {
        return this.loadScalar(
          scalar,
          this.module.local.get(layout.index, binaryen.i32),
        );
      }
      return this.module.local.get(layout.index, this.wasmType(scalar));
    }
    const scalar = this.placeScalarType(place, context);
    return this.loadScalar(scalar, this.placeAddress(place, context));
  }

  storePlace(place, value, scalar, context) {
    if (place.base.kind === "local" && place.projections.length === 0) {
      return this.module.local.set(this.localIndex(place.base.data, context), value);
    }
    if (place.base.kind === "parameter" && place.projections.length === 0) {
      const layout = context.paramLayouts[place.base.data];
      if (layout?.kind !== "scalar_ref") {
        this.fail("assignment to a by-value function parameter is not supported");
      }
      return this.storeScalar(
        scalar,
        this.module.local.get(layout.index, binaryen.i32),
        value,
      );
    }
    return this.storeScalar(scalar, this.placeAddress(place, context), value);
  }

  placeAddress(place, context) {
    let typeId;
    let address;
    switch (place.base.kind) {
      case "parameter": {
        const layout = context.paramLayouts[place.base.data];
        if (!layout || !["scalar_ref", "array_ref"].includes(layout.kind)) {
          this.fail(`parameter id ${place.base.data} is not an addressable reference`);
        }
        typeId = context.function.params[place.base.data].ty;
        address = this.module.local.get(layout.index, binaryen.i32);
        break;
      }
      case "local": {
        const layout =
          this.localArrayLayout[context.functionId]?.[place.base.data]
          ?? this.localScalarRefLayout[context.functionId]?.[place.base.data];
        if (!layout) {
          this.fail(`local id ${place.base.data} is not addressable`);
        }
        typeId = context.function.locals[place.base.data].ty;
        address = this.module.i32.const(layout.address);
        break;
      }
      case "state": {
        const layout = this.stateLayout[place.base.data];
        if (!layout) this.fail(`state id ${place.base.data} is out of range`);
        typeId = this.mir.state[place.base.data].ty;
        address = this.module.i32.add(
          this.module.global.get(POINTER_GLOBALS.state, binaryen.i32),
          this.module.i32.const(layout.offset),
        );
        break;
      }
      case "param": {
        const layout = this.paramLayout[place.base.data];
        if (!layout) this.fail(`param id ${place.base.data} is out of range`);
        typeId = this.mir.interface.params[place.base.data].ty;
        address = this.module.i32.add(
          this.module.global.get(POINTER_GLOBALS.params, binaryen.i32),
          this.module.i32.const(layout.offset),
        );
        break;
      }
      case "event_param": {
        const event = this.mir.interface.events[context.eventId];
        const layout = this.eventLayout[context.eventId]?.[place.base.data];
        if (!event || !layout) {
          this.fail(
            `event parameter id ${place.base.data} is invalid for function '${context.function.name}'`,
          );
        }
        typeId = event.params[place.base.data].ty;
        if (this.type(typeId).kind === "slice") {
          this.fail("slice event parameters require slice-value lowering");
        }
        address = this.compileEventParamAddress(context.eventId, place.base.data);
        break;
      }
      default:
        this.fail(
          `addressable place base '${place.base.kind}' is not in the first Binaryen slice`,
        );
    }

    for (const projection of place.projections) {
      const type = this.type(typeId);
      if (projection.kind !== "index" || type.kind !== "array") {
        this.fail(`projection '${projection.kind}' on '${type.kind}' is not supported yet`);
      }
      const elementLayout = this.typeLayout(type.data.element);
      const index = this.compileBoundedIndex(
        projection.data.index,
        type.data.len,
        projection.data.bounds,
        context,
      );
      address = this.module.i32.add(
        address,
        this.module.i32.mul(index, this.module.i32.const(elementLayout.size)),
      );
      typeId = type.data.element;
    }
    return address;
  }

  compileBoundedIndex(value, length, bounds, context) {
    if (!Number.isInteger(length) || length <= 0) {
      this.fail("array and port lengths must be positive integers");
    }
    if (bounds === "unchecked") {
      return this.compileValue(value, context);
    }
    if (bounds === "clamp") {
      return this.module.select(
        this.module.i32.lt_s(
          this.compileValue(value, context),
          this.module.i32.const(0),
        ),
        this.module.i32.const(0),
        this.module.select(
          this.module.i32.ge_s(
            this.compileValue(value, context),
            this.module.i32.const(length),
          ),
          this.module.i32.const(length - 1),
          this.compileValue(value, context),
        ),
      );
    }
    if (bounds === "checked") {
      const outOfBounds = this.module.i32.or(
        this.module.i32.lt_s(
          this.compileValue(value, context),
          this.module.i32.const(0),
        ),
        this.module.i32.ge_s(
          this.compileValue(value, context),
          this.module.i32.const(length),
        ),
      );
      return this.module.if(
        outOfBounds,
        this.raiseRuntimeFailure(context),
        this.compileValue(value, context),
      );
    }
    this.fail(`unknown bounds mode '${String(bounds)}'`);
  }

  compileDynamicBoundedIndex(
    index,
    length,
    bounds,
    context,
    clampLengthKnownPositive = false,
  ) {
    if (bounds === "unchecked") {
      return index();
    }
    if (bounds === "clamp") {
      const maximum = () =>
        this.module.i32.sub(length(), this.module.i32.const(1));
      const clamped = () =>
        this.module.select(
          this.module.i32.lt_s(index(), this.module.i32.const(0)),
          this.module.i32.const(0),
          this.module.select(
            this.module.i32.gt_s(index(), maximum()),
            maximum(),
            index(),
          ),
        );
      if (clampLengthKnownPositive) return clamped();
      return this.module.if(
        this.module.i32.le_s(length(), this.module.i32.const(0)),
        this.raiseRuntimeFailure(context),
        clamped(),
      );
    }
    if (bounds === "checked") {
      return this.module.if(
        this.module.i32.or(
          this.module.i32.lt_s(index(), this.module.i32.const(0)),
          this.module.i32.ge_s(index(), length()),
        ),
        this.raiseRuntimeFailure(context),
        index(),
      );
    }
    this.fail(`unknown bounds mode '${String(bounds)}'`);
  }

  compileUnary(op, scalar, value) {
    if (!supportsMirOperation("unary", op, scalar)) {
      this.fail(`unary operation '${String(op)}' does not support scalar '${scalar}'`);
    }
    switch (op) {
      case "negate":
        if (scalar === "f32" || scalar === "f64") return this.module[scalar].neg(value);
        return this.module[scalar].sub(this.zero(scalar), value);
      case "logical_not":
        return this.module.i32.eqz(value);
      case "bit_not":
        return this.module[scalar].xor(value, this.minusOne(scalar));
      default:
        this.fail(`unknown unary operation '${String(op)}'`);
    }
  }

  compileBinary(op, scalar, lhs, rhs, context) {
    if (!supportsMirOperation("binary", op, scalar)) {
      this.fail(`binary operation '${String(op)}' does not support scalar '${scalar}'`);
    }
    const wasm = this.module[scalar === "bool" ? "i32" : scalar];
    const integer = scalar === "i32" || scalar === "i64" || scalar === "bool";
    switch (op) {
      case "add": return wasm.add(lhs(), rhs());
      case "subtract": return wasm.sub(lhs(), rhs());
      case "multiply": return wasm.mul(lhs(), rhs());
      case "divide": {
        if (!integer) return wasm.div(lhs(), rhs());
        const minimum = () =>
          scalar === "i64"
            ? this.module.i64.const(-(1n << 63n))
            : this.module.i32.const(-0x8000_0000);
        const negativeOne = () =>
          scalar === "i64"
            ? this.module.i64.const(-1n)
            : this.module.i32.const(-1);
        const overflow = this.module.i32.and(
          wasm.eq(lhs(), minimum()),
          wasm.eq(rhs(), negativeOne()),
        );
        const division = this.module.if(
          overflow,
          minimum(),
          wasm.div_s(lhs(), rhs()),
        );
        return this.module.if(
          wasm.eq(rhs(), this.zero(scalar)),
          this.raiseRuntimeFailure(context),
          division,
        );
      }
      case "remainder":
        if (!integer) {
          return this.compileMathKernelCall("remainder", scalar, [lhs(), rhs()]);
        }
        return this.module.if(
          wasm.eq(rhs(), this.zero(scalar)),
          this.raiseRuntimeFailure(context),
          wasm.rem_s(lhs(), rhs()),
        );
      case "bit_and": return wasm.and(lhs(), rhs());
      case "bit_or": return wasm.or(lhs(), rhs());
      case "bit_xor": return wasm.xor(lhs(), rhs());
      case "shift_left": return wasm.shl(lhs(), rhs());
      case "shift_right": return wasm.shr_s(lhs(), rhs());
      default:
        this.fail(`unknown binary operation '${String(op)}'`);
    }
  }

  compileCompare(op, scalar, lhs, rhs) {
    if (!supportsMirOperation("compare", op, scalar)) {
      this.fail(`comparison '${String(op)}' does not support scalar '${scalar}'`);
    }
    const type = scalar === "bool" ? "i32" : scalar;
    const wasm = this.module[type];
    const integer = type === "i32" || type === "i64";
    switch (op) {
      case "equal": return wasm.eq(lhs, rhs);
      case "not_equal": return wasm.ne(lhs, rhs);
      case "less": return integer ? wasm.lt_s(lhs, rhs) : wasm.lt(lhs, rhs);
      case "less_equal": return integer ? wasm.le_s(lhs, rhs) : wasm.le(lhs, rhs);
      case "greater": return integer ? wasm.gt_s(lhs, rhs) : wasm.gt(lhs, rhs);
      case "greater_equal": return integer ? wasm.ge_s(lhs, rhs) : wasm.ge(lhs, rhs);
      default:
        this.fail(`unknown comparison '${String(op)}'`);
    }
  }

  compileCast(from, to, value) {
    const source = from === "bool" ? "i32" : from;
    const target = to === "bool" ? "i32" : to;
    if (source === target) return value;
    if (target === "bool") return this.module[source].ne(value, this.zero(source));
    if (source === "f32" && target === "f64") return this.module.f64.promote(value);
    if (source === "f64" && target === "f32") return this.module.f32.demote(value);
    if (source === "i32" && target === "i64") return this.module.i64.extend_s(value);
    if (source === "i64" && target === "i32") return this.module.i32.wrap(value);
    if ((source === "i32" || source === "i64") && target === "f32") {
      return this.module.f32.convert_s[source](value);
    }
    if ((source === "i32" || source === "i64") && target === "f64") {
      return this.module.f64.convert_s[source](value);
    }
    if ((source === "f32" || source === "f64") && target === "i32") {
      this.module.setFeatures(
        this.module.getFeatures() | binaryen.Features.NontrappingFPToInt,
      );
      return this.module.i32.trunc_s_sat[source](value);
    }
    if ((source === "f32" || source === "f64") && target === "i64") {
      this.module.setFeatures(
        this.module.getFeatures() | binaryen.Features.NontrappingFPToInt,
      );
      return this.module.i64.trunc_s_sat[source](value);
    }
    this.fail(`unsupported scalar cast from '${from}' to '${to}'`);
  }

  compileIntrinsic(data, expectedScalar, context) {
    const scalar = data.args.length
      ? this.valueScalarType(data.args[0], context)
      : expectedScalar;
    const args = data.args.map((value) => this.compileValue(value, context));
    const isFloat = scalar === "f32" || scalar === "f64";
    const isInteger = scalar === "i32" || scalar === "i64";
    if (!isFloat && !isInteger) {
      this.fail(`intrinsic '${data.intrinsic}' requires numeric operands`);
    }
    const wasm = this.module[scalar];

    if (isInteger) {
      // Binaryen expressions are tree nodes, so each use needs its own local
      // get/constant node even when the MIR value is the same.
      const arg = (index) => this.compileValue(data.args[index], context);
      switch (data.intrinsic) {
        case "abs":
          return this.module.select(
            wasm.lt_s(arg(0), this.zero(scalar)),
            wasm.sub(this.zero(scalar), arg(0)),
            arg(0),
          );
        case "min":
          return this.module.select(wasm.lt_s(arg(0), arg(1)), arg(0), arg(1));
        case "max":
          return this.module.select(wasm.gt_s(arg(0), arg(1)), arg(0), arg(1));
        case "range_clamp":
          return this.module.select(
            wasm.lt_s(arg(0), arg(1)),
            arg(1),
            this.module.select(
              wasm.gt_s(arg(0), arg(2)),
              arg(2),
              arg(0),
            ),
          );
        case "range_wrap": { // Bounds are validator-required integer constants.
          const lowerLiteral = data.args[1]?.data;
          const upperLiteral = data.args[2]?.data;
          if (
            data.args[1]?.kind !== "constant"
            || data.args[2]?.kind !== "constant"
            || lowerLiteral?.type !== scalar
            || upperLiteral?.type !== scalar
          ) {
            this.fail("range_wrap requires constant bounds matching its integer operand");
          }
          const lower = scalar === "i64"
            ? decodeI64Literal(lowerLiteral.value, this)
            : BigInt(lowerLiteral.value);
          const upper = scalar === "i64"
            ? decodeI64Literal(upperLiteral.value, this)
            : BigInt(upperLiteral.value);
          const bits = scalar === "i64" ? 64n : 32n;
          const width = upper - lower + 1n;
          if (width === (1n << bits)) return arg(0);
          const encodedWidth = BigInt.asIntN(Number(bits), width);
          const widthValue = scalar === "i64"
            ? this.module.i64.const(encodedWidth)
            : this.module.i32.const(Number(encodedWidth));
          const encodedSpan = BigInt.asIntN(Number(bits), width - 1n);
          const spanValue = scalar === "i64"
            ? this.module.i64.const(encodedSpan)
            : this.module.i32.const(Number(encodedSpan));
          const one = scalar === "i64"
            ? this.module.i64.const(1n)
            : this.module.i32.const(1);
          const distanceFromLower = lower === 0n
            ? arg(0)
            : wasm.sub(arg(0), arg(1));
          return this.module.if(
            wasm.le_u(distanceFromLower, spanValue),
            arg(0),
            this.module.if(
              wasm.lt_s(arg(0), arg(1)),
              wasm.sub(
                arg(2),
                wasm.rem_u(
                  wasm.sub(wasm.sub(arg(1), one), arg(0)),
                  widthValue,
                ),
              ),
              wasm.add(
                arg(1),
                wasm.rem_u(
                  wasm.sub(arg(0), wasm.add(arg(2), one)),
                  widthValue,
                ),
              ),
            ),
          );
        }
        default:
          this.fail(`intrinsic '${data.intrinsic}' requires f32 or f64 operands`);
      }
    }

    switch (data.intrinsic) {
      case "sqrt": return wasm.sqrt(args[0]);
      case "abs": return wasm.abs(args[0]);
      case "floor": return wasm.floor(args[0]);
      case "ceil": return wasm.ceil(args[0]);
      case "trunc": return wasm.trunc(args[0]);
      case "min": return wasm.min(args[0], args[1]);
      case "max": return wasm.max(args[0], args[1]);
      case "range_clamp": {
        const arg = (index) => this.compileValue(data.args[index], context);
        return this.module.select(
          wasm.ne(arg(0), arg(0)),
          arg(1),
          this.module.select(
            wasm.lt(arg(0), arg(1)),
            arg(1),
            this.module.select(
              wasm.gt(arg(0), arg(2)),
              arg(2),
              arg(0),
            ),
          ),
        );
      }
      case "fma": return this.compileMathKernelCall(data.intrinsic, scalar, args);
      case "sin":
      case "cos":
      case "tan":
      case "tanh":
      case "atan":
      case "atan2":
      case "exp":
      case "log":
      case "pow":
        return this.compileMathKernelCall(data.intrinsic, scalar, args);
      case "round":
        return this.compileRoundHelper(scalar, args);
      default:
        this.fail(`unknown intrinsic '${String(data.intrinsic)}'`);
    }
  }

  compileMathKernelCall(intrinsic, scalar, args) {
    const name = `onda_math_${intrinsic}_${scalar}`;
    if (!this.requiredMathHelpers.has(name)) {
      this.fail(`math kernel was not reserved for helper '${name}'`);
    }
    return this.module.call(
      name,
      args,
      this.wasmType(scalar),
    );
  }

  compileRoundHelper(scalar, args) {
    const name = `$onda.math.round.${scalar}`;
    if (!this.internalHelpers.has(name)) {
      const wasm = this.module[scalar];
      const get = () => this.module.local.get(0, this.wasmType(scalar));
      const trunc = () => wasm.trunc(get());
      const magnitude = wasm.abs(wasm.sub(get(), trunc()));
      const rounded = wasm.add(
        trunc(),
        wasm.copysign(wasm.const(1), get()),
      );
      this.module.addFunction(
        name,
        this.wasmType(scalar),
        this.wasmType(scalar),
        [],
        this.module.select(
          wasm.ge(magnitude, wasm.const(0.5)),
          rounded,
          trunc(),
        ),
      );
      this.internalHelpers.add(name);
    }
    return this.module.call(name, args, this.wasmType(scalar));
  }

  loadScalar(scalar, address) {
    switch (scalar) {
      case "bool": return this.module.i32.load8_u(0, 1, address);
      case "i32": return this.module.i32.load(0, 4, address);
      case "i64": return this.module.i64.load(0, 8, address);
      case "f32": return this.module.f32.load(0, 4, address);
      case "f64": return this.module.f64.load(0, 8, address);
      default: this.fail(`unknown scalar type '${String(scalar)}'`);
    }
  }

  storeScalar(scalar, address, value) {
    switch (scalar) {
      case "bool": return this.module.i32.store8(0, 1, address, value);
      case "i32": return this.module.i32.store(0, 4, address, value);
      case "i64": return this.module.i64.store(0, 8, address, value);
      case "f32": return this.module.f32.store(0, 4, address, value);
      case "f64": return this.module.f64.store(0, 8, address, value);
      default: this.fail(`unknown scalar type '${String(scalar)}'`);
    }
  }

  storePackedScalar(scalar, address, value) {
    switch (scalar) {
      case "bool": return this.module.i32.store8(0, 1, address, value);
      case "i32": return this.module.i32.store(0, 1, address, value);
      case "i64": return this.module.i64.store(0, 1, address, value);
      case "f32": return this.module.f32.store(0, 1, address, value);
      case "f64": return this.module.f64.store(0, 1, address, value);
      default: this.fail(`unknown scalar type '${String(scalar)}'`);
    }
  }

  type(typeId) {
    const type = this.mir.types[typeId];
    if (!type) this.fail(`type id ${typeId} is out of range`);
    return type;
  }

  typesEquivalent(lhsId, rhsId, visiting = new Set()) {
    if (lhsId === rhsId) return true;
    const key = `${lhsId}:${rhsId}`;
    if (visiting.has(key)) return true;
    const lhs = this.mir.types[lhsId];
    const rhs = this.mir.types[rhsId];
    if (!lhs || !rhs || lhs.kind !== rhs.kind) return false;
    visiting.add(key);
    let equivalent = false;
    if (lhs.kind === "scalar") {
      equivalent = lhs.data === rhs.data;
    } else if (lhs.kind === "array") {
      equivalent =
        lhs.data.len === rhs.data.len &&
        this.typesEquivalent(lhs.data.element, rhs.data.element, visiting);
    } else if (lhs.kind === "slice") {
      equivalent =
        lhs.data.element === rhs.data.element &&
        lhs.data.access === rhs.data.access;
    } else if (lhs.kind === "buffer") {
      equivalent =
        lhs.data.element === rhs.data.element &&
        JSON.stringify(lhs.data.channels) === JSON.stringify(rhs.data.channels) &&
        lhs.data.access === rhs.data.access;
    } else if (lhs.kind === "buffer_span") {
      equivalent =
        lhs.data.element === rhs.data.element &&
        JSON.stringify(lhs.data.channels) === JSON.stringify(rhs.data.channels) &&
        lhs.data.access === rhs.data.access &&
        lhs.data.len === rhs.data.len;
    } else if (lhs.kind === "tuple") {
      equivalent =
        lhs.data.length === rhs.data.length &&
        lhs.data.every((element, index) =>
          this.typesEquivalent(element, rhs.data[index], visiting),
        );
    } else if (lhs.kind === "struct") {
      equivalent = lhs.data === rhs.data;
    }
    visiting.delete(key);
    return equivalent;
  }

  typeLayout(typeId) {
    const type = this.type(typeId);
    if (type.kind === "scalar") {
      const size = this.scalarSize(type.data);
      return { size, align: size, scalar: type.data };
    }
    if (type.kind === "array") {
      const element = this.typeLayout(type.data.element);
      if (!Number.isInteger(type.data.len) || type.data.len <= 0) {
        this.fail("fixed array length must be a positive integer");
      }
      return {
        size: element.size * type.data.len,
        align: element.align,
        scalar: element.scalar,
      };
    }
    this.fail(`storage layout for MIR type '${type.kind}' is not supported yet`);
  }

  requireScalarType(typeId, description) {
    const type = this.type(typeId);
    if (type.kind !== "scalar") {
      this.fail(`${description} has unsupported non-scalar type '${type.kind}'`);
    }
    return type.data;
  }

  scalarSize(scalar) {
    switch (scalar) {
      case "bool": return 1;
      case "i32":
      case "f32": return 4;
      case "i64":
      case "f64": return 8;
      default: this.fail(`unknown scalar type '${String(scalar)}'`);
    }
  }

  requireWasm32Extent(byteLength, description) {
    if (
      !Number.isSafeInteger(byteLength)
      || byteLength < 0
      || byteLength >= WASM32_ADDRESS_SPACE_BYTES
    ) {
      this.fail(`${description} must fit within the wasm32 4 GiB address space`);
    }
  }

  wasmType(scalar) {
    return binaryen[scalar === "bool" ? "i32" : scalar];
  }

  wasmResultType(scalars) {
    if (scalars.length === 0) return binaryen.none;
    if (scalars.length === 1) return this.wasmType(scalars[0]);
    return binaryen.createType(scalars.map((scalar) => this.wasmType(scalar)));
  }

  zero(scalar) {
    const type = scalar === "bool" ? "i32" : scalar;
    return type === "i64"
      ? this.module.i64.const(0n)
      : this.module[type].const(0);
  }

  minusOne(scalar) {
    const type = scalar === "bool" ? "i32" : scalar;
    return type === "i64"
      ? this.module.i64.const(-1n)
      : this.module.i32.const(-1);
  }

  localIndex(localId, context) {
    if (!Number.isInteger(localId) || localId < 0 || localId >= context.localScalars.length) {
      this.fail(`local id ${localId} is out of range`);
    }
    return context.localLayouts[localId].index;
  }

  requireFunctionId(id, description) {
    if (!Number.isInteger(id) || id < 0 || id >= this.mir.functions.length) {
      this.fail(`${description} function id ${id} is out of range`);
    }
  }

  requireBuffer(id) {
    if (!Number.isInteger(id) || id < 0 || id >= this.mir.interface.buffers.length) {
      this.fail(`buffer id ${id} is out of range`);
    }
    return this.mir.interface.buffers[id];
  }

  bufferRefFirst(bufferRef) {
    if (Number.isInteger(bufferRef)) return bufferRef;
    if (bufferRef?.kind === "direct" && Number.isInteger(bufferRef.data)) {
      return bufferRef.data;
    }
    if (
      bufferRef?.kind === "array_element"
      && Number.isInteger(bufferRef.data?.first)
      && Number.isInteger(bufferRef.data?.len)
      && bufferRef.data.len > 0
    ) {
      return bufferRef.data.first;
    }
    this.fail("invalid MIR buffer reference");
  }

  requireBufferRef(bufferRef) {
    const first = this.bufferRefFirst(bufferRef);
    const buffer = this.requireBuffer(first);
    if (bufferRef?.kind === "array_element") {
      const last = first + bufferRef.data.len - 1;
      this.requireBuffer(last);
      for (let id = first + 1; id <= last; id += 1) {
        const candidate = this.requireBuffer(id);
        if (
          candidate.element !== buffer.element
          || JSON.stringify(candidate.channels) !== JSON.stringify(buffer.channels)
          || candidate.access !== buffer.access
        ) {
          this.fail("buffer-array elements must have one descriptor type");
        }
      }
    }
    return buffer;
  }

  bufferRefChannelMetadata(bufferRef) {
    const buffer = this.requireBufferRef(bufferRef);
    return this.bufferChannelMetadata(buffer.channels, buffer.element);
  }

  compileBufferRefIndex(bufferRef, context) {
    const staticIndex = this.staticBufferRefIndex(bufferRef);
    if (staticIndex !== null) return this.module.i32.const(staticIndex);
    const first = this.bufferRefFirst(bufferRef);
    const data = bufferRef.data;
    const selector = this.compileDynamicBoundedIndex(
      () => this.compileValue(data.selector, context),
      () => this.module.i32.const(data.len),
      data.bounds,
      context,
      true,
    );
    return this.module.i32.add(this.module.i32.const(first), selector);
  }

  staticBufferRefIndex(bufferRef) {
    const first = this.bufferRefFirst(bufferRef);
    if (Number.isInteger(bufferRef) || bufferRef.kind === "direct") return first;
    const data = bufferRef.data;
    const selector = data.selector;
    if (
      selector?.kind !== "constant"
      || selector.data?.type !== "i32"
      || !Number.isInteger(selector.data.value)
    ) {
      return null;
    }
    let index = selector.data.value;
    if (data.bounds === "clamp") {
      index = Math.min(data.len - 1, Math.max(0, index));
    } else if (index < 0 || index >= data.len) {
      // Preserve checked failure behavior and leave invalid unchecked MIR to
      // the normal validated lowering path.
      return null;
    }
    return first + index;
  }

  currentLabel(labels, statement) {
    const label = labels.at(-1);
    if (!label) this.fail(`'${statement}' appears outside a MIR loop`);
    return label;
  }

  buildMetadata() {
    const stateSize = this.stateLayout.byteLength ?? 0;
    const paramSize = this.paramLayout.byteLength ?? 0;
    const snapshot = this.stateSnapshotMetadata();
    const eventExports = this.mir.interface.events.map(
      (_, id) => `onda_event_${id}`,
    );
    const requiredExports = [
      "memory",
      "__heap_base",
      "onda_processor_init",
      "onda_process",
      ...eventExports,
    ];
    const targetFeatures = ["bulk-memory"];
    if (this.options.simd) targetFeatures.push("simd128");
    return {
      format: PROCESSOR_ARTIFACT_FORMAT,
      format_version: PROCESSOR_ARTIFACT_FORMAT_VERSION,
      artifact_kind: "webassembly_module",
      abi_version: PROCESSOR_ABI_VERSION,
      backend: "binaryen-js",
      mir_schema_version: this.mir.schema_version,
      target: {
        triple: "wasm32-unknown-unknown",
        cpu: "generic",
        features: targetFeatures.map((feature) => `+${feature}`).join(","),
        reloc_model: "static",
        code_model: "default",
        opt_level: String(this.options.optimizeLevel),
        abi_name: null,
        data_layout: "e-m:e-p:32:32-i64:64-n32:64-S128",
        pointer_width_bits: 32,
        byte_order: "little_endian",
        pointer_model: "linear_memory_offset",
        calling_convention: "core_webassembly",
      },
      integration: {
        required_symbols: requiredExports,
        one_processor_per_artifact: true,
        profile: {
          kind: "core_webassembly_module",
          imports: [],
          memory_export: "memory",
          heap_base_export: "__heap_base",
        },
      },
      required_features: targetFeatures,
      optimization: {
        enabled: this.options.optimize,
        level: this.options.optimizeLevel,
        shrink_level: this.options.shrinkLevel,
        fast_math: this.options.fastMath,
        simd: this.options.simd,
        inline_functions_with_loops:
          this.options.allowInliningFunctionsWithLoops,
      },
      compile: {
        sample_rate: this.mir.config.sample_rate,
        block_size: this.mir.config.block_size,
        fast_math: this.options.fastMath,
      },
      exports: {
        memory: "memory",
        heap_base: "__heap_base",
        init: "onda_processor_init",
        process: "onda_process",
        events: eventExports,
      },
      runtime: {
        state_size_bytes: stateSize,
        state_align_bytes: 16,
        state_initialization: "zeroed",
        snapshot_size_bytes: snapshot.byteLength,
        snapshot_format_version: PROCESSOR_SNAPSHOT_FORMAT_VERSION,
        snapshot_byte_order: "little_endian",
        snapshot_restore_base: "post_init_physical_state_image",
        param_size_bytes: paramSize,
        param_align_bytes: 16,
        requires_full_blocks: false,
        delegate_record_header_size_bytes: DELEGATE_RECORD_HEADER_SIZE,
        print_record_header_size_bytes: PRINT_RECORD_HEADER_SIZE,
      },
      metadata: {
        source_files: (this.mir.source_files ?? []).map((file) => ({
          path: file.path,
        })),
        log_sites: (this.mir.log_sites ?? []).map((site, index) => ({
          index,
          label: site.label ?? null,
          source: {
            file: site.source.file ?? null,
            line: site.source.line,
            column: site.source.column,
            end_line: site.source.end_line,
            end_column: site.source.end_column,
          },
          lexical_owner: site.lexical_owner,
          declaration: site.declaration ?? null,
          argument_types: [...site.argument_types],
          payload_size_bytes: site.payload_size,
        })),
        states: snapshot.entries,
        inputs: this.portMetadata(this.mir.interface.inputs, this.inputLayout),
        outputs: this.portMetadata(this.mir.interface.outputs, this.outputLayout),
        control_outputs: this.mir.interface.control_outputs.map((output, id) => ({
          name: output.name,
          type_repr: typeName(this.type(output.ty), this),
          scalar: this.storageShape(output.ty).scalar,
          array_len: this.storageShape(output.ty).length,
          element_size_bytes: this.scalarSize(this.storageShape(output.ty).scalar),
          slot_offset: this.interfaceSlotOffset(
            this.mir.interface.control_outputs,
            id,
          ),
          byte_offset: null,
          state_byte_offset: this.controlOutputLayout[id].offset,
          byte_size: this.controlOutputLayout[id].size,
          default_reprs: null,
          range_min_repr: null,
          range_max_repr: null,
          param_control: null,
        })),
        params: this.mir.interface.params.map((param, id) => ({
          name: param.name,
          type_repr: typeName(this.type(param.ty), this),
          scalar: this.storageShape(param.ty).scalar,
          array_len: this.storageShape(param.ty).length,
          element_size_bytes: this.scalarSize(this.storageShape(param.ty).scalar),
          slot_offset: this.interfaceSlotOffset(this.mir.interface.params, id),
          byte_offset: this.paramLayout[id].offset,
          state_byte_offset: null,
          byte_size: this.paramLayout[id].size,
          default_reprs: this.constantReprs(param.default),
          range_min_repr: this.scalarRepr(param.range?.min),
          range_max_repr: this.scalarRepr(param.range?.max),
          param_control: this.storageShape(param.ty).length === 1 && param.range
            ? {
                scale: param.control.scale,
                curve: param.control.curve,
                unit: param.control.unit,
                step_repr: this.scalarRepr(param.control.step),
                step_count: param.control.step_count,
              }
            : null,
        })),
        buffers: this.mir.interface.buffers.map((buffer, bufferId) => {
          const channels = this.bufferChannelMetadata(buffer.channels, buffer.element);
          return {
            name: buffer.name,
            type_repr: this.bufferTypeRepr(buffer, channels),
            scalar: buffer.element,
            element_size_bytes: this.scalarSize(buffer.element),
            channels: channels.kind,
            static_channels: channels.count,
            access: buffer.access,
            may_write: this.bufferMayWrite[bufferId],
          };
        }),
        buffer_arrays: (this.mir.interface.buffer_arrays ?? []).map((array) => ({
          name: array.name,
          first_buffer: array.first,
          len: array.len,
        })),
        events: this.mir.interface.events.map((event, eventId) => ({
          name: event.name,
          export: `onda_event_${eventId}`,
          payload_size_bytes: this.eventLayout[eventId].byteLength,
          payload_min_size_bytes: this.eventLayout[eventId].minimumByteLength,
          has_dynamic_payload: this.eventLayout[eventId].dynamic,
          params: event.params.map((param, paramId) => {
            const shape = this.storageShape(param.ty);
            return {
              name: param.name,
              type_repr: typeName(this.type(param.ty), this),
              scalar: shape.scalar,
              array_len: shape.length ?? 0,
              is_array: shape.isArray === true && shape.isSlice !== true,
              is_slice: shape.isSlice === true,
              byte_offset: this.eventLayout[eventId][paramId].offset,
              byte_size: this.eventLayout[eventId][paramId].size,
              element_size_bytes: this.scalarSize(shape.scalar),
              has_default: param.default !== null && param.default !== undefined,
              default_reprs: this.constantReprs(param.default),
            };
          }),
        })),
        delegates: this.mir.interface.delegates.map((delegate, delegateId) => ({
          index: delegateId,
          name: delegate.name,
          payload_size_bytes: this.delegateLayout[delegateId].byteLength,
          payload_min_size_bytes: this.delegateLayout[delegateId].minimumByteLength,
          has_dynamic_payload: this.delegateLayout[delegateId].dynamic,
          params: delegate.params.map((param, paramId) => {
            const shape = this.storageShape(param.ty);
            return {
              name: param.name,
              type_repr: typeName(this.type(param.ty), this),
              scalar: shape.scalar,
              array_len: shape.length ?? 0,
              is_array: shape.isArray === true && shape.isSlice !== true,
              is_slice: shape.isSlice === true,
              byte_offset: this.delegateLayout[delegateId][paramId].offset,
              byte_size: this.delegateLayout[delegateId][paramId].size,
              element_size_bytes: this.scalarSize(shape.scalar),
            };
          }),
        })),
      },
    };
  }

  stateSnapshotMetadata() {
    let byteOffset = 0;
    const entries = [];
    for (const [id, slot] of this.mir.state.entries()) {
      if (slot.persistence !== "snapshot") {
        continue;
      }
      const shape = this.storageShape(slot.ty);
      const layout = this.stateLayout[id];
      entries.push({
        name: slot.name,
        authored: slot.authored,
        type_repr: typeName(this.type(slot.ty), this),
        scalar: shape.scalar,
        array_len: shape.length,
        element_size_bytes: this.scalarSize(shape.scalar),
        packed_snapshot_byte_offset: byteOffset,
        physical_state_byte_offset: layout.offset,
        byte_size: layout.size,
        integer_range: slot.integer_range === undefined || slot.integer_range === null
          ? null
          : {
              min: {
                type: slot.integer_range.min.type,
                value: String(slot.integer_range.min.value),
              },
              max: {
                type: slot.integer_range.max.type,
                value: String(slot.integer_range.max.value),
              },
              mode: slot.integer_range.mode,
            },
      });
      byteOffset += layout.size;
    }
    return { entries, byteLength: byteOffset };
  }

  portMetadata(ports, layouts) {
    return ports.map((port, id) => ({
      name: port.name,
      type_repr: typeName(this.type(port.ty), this),
      scalar: layouts[id].scalar,
      array_len: layouts[id].channels,
      element_size_bytes: layouts[id].size,
      slot_offset: layouts[id].channel,
      byte_offset: null,
      state_byte_offset: null,
      byte_size: layouts[id].size * layouts[id].channels,
      default_reprs: null,
      range_min_repr: null,
      range_max_repr: null,
      param_control: null,
    }));
  }

  interfaceSlotOffset(values, end) {
    let offset = 0;
    for (let id = 0; id < end; id += 1) {
      offset += this.storageShape(values[id].ty).length;
    }
    return offset;
  }

  constantReprs(value) {
    if (value === null || value === undefined) return null;
    if (value.kind === "scalar") return [this.scalarRepr(value.data)];
    if (value.kind === "aggregate") {
      return value.data.flatMap((entry) => this.constantReprs(entry) ?? []);
    }
    this.fail(`unknown MIR constant kind '${String(value.kind)}'`);
  }

  scalarRepr(value) {
    if (value === null || value === undefined) return null;
    if (Object.is(value.value, -0)) return "-0";
    return String(value.value);
  }

  bufferTypeRepr(buffer, channels) {
    if (channels.kind === "mono") return `buffer<${buffer.element}>`;
    if (channels.kind === "static") {
      return `buffer<${buffer.element}[${channels.count}]>`;
    }
    return `buffer<${buffer.element}[]>`;
  }

  storageShape(typeId) {
    const type = this.type(typeId);
    if (type.kind === "scalar") {
      return { scalar: type.data, length: 1, isArray: false };
    }
    if (type.kind === "array") {
      const element = this.type(type.data.element);
      if (element.kind !== "scalar") {
        this.fail("nested aggregate storage metadata is not supported yet");
      }
      return { scalar: element.data, length: type.data.len, isArray: true };
    }
    if (type.kind === "slice") {
      return {
        scalar: type.data.element,
        length: null,
        isArray: true,
        isSlice: true,
      };
    }
    this.fail(`storage metadata for MIR type '${type.kind}' is not supported yet`);
  }

  bufferChannelMetadata(channels, element) {
    if (channels === "mono") {
      return { kind: "mono", count: 1 };
    }
    if (channels === "dynamic") {
      return { kind: "dynamic", count: null };
    }
    if (
      channels &&
      typeof channels === "object" &&
      Number.isInteger(channels.static) &&
      channels.static > 0 &&
      channels.static <= Math.floor(0x7fffffff / this.scalarSize(element))
    ) {
      return { kind: "static", count: channels.static };
    }
    this.fail(`invalid MIR buffer channel descriptor '${JSON.stringify(channels)}'`);
  }

  fail(message) {
    throw new OndaBinaryenError(message);
  }
}
