(() => {
  const GLOBAL_NAME = "__ONDA_PARAM_CONTROL_V2__";
  if (globalThis[GLOBAL_NAME]) return;
  const SCALES = Object.freeze(["linear", "log"]);

  function fail(param, message) {
    const name = typeof param?.name === "string" ? ` '${param.name}'` : "";
    throw new TypeError(`Onda parameter${name} ${message}`);
  }

  function scalarKind(param) {
    if (!param || typeof param !== "object" || Array.isArray(param)) {
      fail(param, "metadata must be an object");
    }
    if (Number(param.array_len) !== 1) {
      fail(param, "does not have a scalar host-control domain");
    }
    if (!["bool", "f32", "f64", "i32", "i64"].includes(param.scalar)) {
      fail(param, `has unsupported scalar type '${String(param.scalar)}'`);
    }
    return param.scalar;
  }

  function numericDomain(param) {
    const scalar = scalarKind(param);
    if (scalar === "bool") return null;
    const control = param.param_control;
    if (!control || typeof control !== "object" || Array.isArray(control)) {
      fail(param, "has no numeric host-control domain");
    }
    const minimum = Number(param.range_min_repr);
    const maximum = Number(param.range_max_repr);
    if (!Number.isFinite(minimum) || !Number.isFinite(maximum) || minimum >= maximum) {
      fail(param, "has invalid host-control range metadata");
    }
    if (!SCALES.includes(control.scale)) {
      fail(param, `has unsupported control scale '${String(control.scale)}'`);
    }
    if (control.scale === SCALES[1] && minimum <= 0) {
      fail(param, "has a non-positive logarithmic host-control range");
    }
    let step = null;
    if (control.step_repr !== null) {
      step = Number(control.step_repr);
      if (!Number.isFinite(step) || step <= 0) {
        fail(param, "has invalid host-control step metadata");
      }
    }
    return { minimum, maximum, scale: control.scale, step };
  }

  function constrainParamPlain(param, plain) {
    if (scalarKind(param) === "bool") return Boolean(plain);
    const { minimum, maximum, step } = numericDomain(param);
    const numeric = Number(plain);
    let constrained = Number.isNaN(numeric)
      ? minimum
      : Math.min(maximum, Math.max(minimum, numeric));
    if (step !== null) {
      constrained = minimum + Math.round((constrained - minimum) / step) * step;
      constrained = Math.min(maximum, Math.max(minimum, constrained));
    }
    return constrained;
  }

  function paramNormalizedToPlain(param, normalized) {
    if (scalarKind(param) === "bool") return Number(normalized) >= 0.5;
    const { minimum, maximum, scale } = numericDomain(param);
    const numeric = Number(normalized);
    const unit = Number.isNaN(numeric)
      ? 0
      : Math.min(1, Math.max(0, numeric));
    if (unit === 0) return minimum;
    if (unit === 1) return maximum;
    const plain = scale === SCALES[1]
      ? minimum * ((maximum / minimum) ** unit)
      : minimum + unit * (maximum - minimum);
    return constrainParamPlain(param, plain);
  }

  function paramPlainToNormalized(param, plain) {
    if (scalarKind(param) === "bool") return Boolean(plain) ? 1 : 0;
    const { minimum, maximum, scale } = numericDomain(param);
    const constrained = constrainParamPlain(param, plain);
    if (constrained === minimum) return 0;
    if (constrained === maximum) return 1;
    const normalized = scale === SCALES[1]
      ? Math.log(constrained / minimum) / Math.log(maximum / minimum)
      : (constrained - minimum) / (maximum - minimum);
    return Math.min(1, Math.max(0, normalized));
  }

  Object.defineProperty(globalThis, GLOBAL_NAME, {
    configurable: false,
    enumerable: false,
    writable: false,
    value: Object.freeze({
      scales: SCALES,
      constrainParamPlain,
      paramNormalizedToPlain,
      paramPlainToNormalized,
    }),
  });
})();
