(() => {
  const GLOBAL_NAME = "__ONDA_PARAM_CONTROL_V2__";
  if (globalThis[GLOBAL_NAME]) return;
  const SCALES = Object.freeze(["linear", "log"]);
  const INTEGER_REPR = /^-?(0|[1-9][0-9]*)$/;
  const I32_MIN = -2147483648n;
  const I32_MAX = 2147483647n;
  const I64_MIN = -(1n << 63n);
  const I64_MAX = (1n << 63n) - 1n;
  const MAX_EXACT_HOST_INTEGER = BigInt(Number.MAX_SAFE_INTEGER);
  const CURVE_LINEAR_EPSILON = 0.001;

  function fail(param, message) {
    const name = typeof param?.name === "string" ? ` '${param.name}'` : "";
    throw new TypeError(`Onda parameter${name} ${message}`);
  }

  function scalarKind(param) {
    if (!param || typeof param !== "object" || Array.isArray(param)) {
      fail(param, "metadata must be an object");
    }
    if (
      Number(param.array_len) !== 1
      || typeof param.scalar !== "string"
      || param.type_repr !== param.scalar
    ) {
      fail(param, "does not have a scalar host-control domain");
    }
    if (!["bool", "f32", "f64", "i32", "i64"].includes(param.scalar)) {
      fail(param, `has unsupported scalar type '${String(param.scalar)}'`);
    }
    return param.scalar;
  }

  function integerValue(param, value, field, minimum, maximum) {
    if (typeof value !== "string" || !INTEGER_REPR.test(value)) {
      fail(param, `has invalid ${field} metadata`);
    }
    let decoded;
    try {
      decoded = BigInt(value);
    } catch {
      fail(param, `has invalid ${field} metadata`);
    }
    if (decoded < minimum || decoded > maximum) {
      fail(param, `has ${field} metadata outside its scalar range`);
    }
    return decoded;
  }

  function scalarMetadataValue(param, value, field) {
    if (param.scalar === "i32") {
      return integerValue(param, value, field, I32_MIN, I32_MAX);
    }
    if (param.scalar === "i64") {
      return integerValue(param, value, field, I64_MIN, I64_MAX);
    }
    if (typeof value !== "string") {
      fail(param, `has invalid ${field} metadata`);
    }
    const decoded = Number(value);
    if (!Number.isFinite(decoded)) {
      fail(param, `has invalid ${field} metadata`);
    }
    return decoded;
  }

  function numericDomain(param) {
    const scalar = scalarKind(param);
    if (scalar === "bool") return null;
    const control = param.param_control;
    if (!control || typeof control !== "object" || Array.isArray(control)) {
      fail(param, "has no numeric host-control domain");
    }
    if (control.unit !== null && typeof control.unit !== "string") {
      fail(param, "has invalid host-control unit metadata");
    }
    if (control.unit?.includes("\0")) {
      fail(param, "has a host-control unit containing a NUL character");
    }
    const integer = scalar === "i32" || scalar === "i64";
    const minimumValue = scalarMetadataValue(
      param,
      param.range_min_repr,
      "range minimum",
    );
    const maximumValue = scalarMetadataValue(
      param,
      param.range_max_repr,
      "range maximum",
    );
    const integerMinimum = integer ? minimumValue : null;
    const integerMaximum = integer ? maximumValue : null;
    if (integer && integerMinimum >= integerMaximum) {
      fail(param, "has invalid host-control range metadata");
    }
    if (
      scalar === "i64"
      && (
        integerMinimum < -MAX_EXACT_HOST_INTEGER
        || integerMaximum > MAX_EXACT_HOST_INTEGER
        || integerMaximum - integerMinimum > MAX_EXACT_HOST_INTEGER
      )
    ) {
      fail(param, "has an i64 domain outside the exact host-control integer range");
    }
    const minimum = integer ? Number(integerMinimum) : minimumValue;
    const maximum = integer ? Number(integerMaximum) : maximumValue;
    if (!Number.isFinite(minimum) || !Number.isFinite(maximum) || minimum >= maximum) {
      fail(param, "has invalid host-control range metadata");
    }
    if (!SCALES.includes(control.scale)) {
      fail(param, `has unsupported control scale '${String(control.scale)}'`);
    }
    if (control.curve !== null && !Number.isFinite(control.curve)) {
      fail(param, "has invalid host-control curve metadata");
    }
    if (control.scale === SCALES[1] && control.curve !== null) {
      fail(param, "combines logarithmic scale with curve");
    }
    if (control.scale === SCALES[1] && scalar !== "f32" && scalar !== "f64") {
      fail(param, "uses logarithmic scale with a non-floating scalar");
    }
    if (control.scale === SCALES[1] && minimum <= 0) {
      fail(param, "has a non-positive logarithmic host-control range");
    }
    if (
      control.scale === SCALES[1]
      && (control.step_repr !== null || control.step_count !== null)
    ) {
      fail(param, "combines logarithmic scale with a step");
    }
    if ((control.step_repr === null) !== (control.step_count === null)) {
      fail(param, "must provide step and step_count together");
    }
    let step = null;
    let integerStep = null;
    if (control.step_repr !== null) {
      const stepValue = scalarMetadataValue(param, control.step_repr, "step");
      integerStep = integer ? stepValue : null;
      step = integer ? Number(integerStep) : stepValue;
      if (!Number.isFinite(step) || step <= 0) {
        fail(param, "has invalid host-control step metadata");
      }
      if (
        !Number.isSafeInteger(control.step_count)
        || control.step_count <= 0
        || control.step_count > 0xffff_ffff
      ) {
        fail(param, "has invalid host-control step_count metadata");
      }
      if (integer) {
        const width = integerMaximum - integerMinimum;
        if (
          width % integerStep !== 0n
          || width / integerStep !== BigInt(control.step_count)
        ) {
          fail(param, "has step_count inconsistent with its range and step");
        }
      } else {
        const tolerance = scalar === "f32" ? 1.0e-5 : 1.0e-10;
        const ratio = (maximum - minimum) / step;
        if (
          !Number.isFinite(ratio)
          || Math.abs(ratio - control.step_count)
            > tolerance * Math.max(Math.abs(ratio), 1)
        ) {
          fail(param, "has step_count inconsistent with its range and step");
        }
      }
    } else if (integer) {
      fail(param, "has an integer range without a step");
    }
    return {
      minimum,
      maximum,
      scale: control.scale,
      curve: control.curve,
      step,
      integerMinimum,
      integerMaximum,
      integerStep,
    };
  }

  function validateParamControlDomain(param, requireDefault = false) {
    if (scalarKind(param) === "bool") {
      fail(param, "does not have a numeric host-control domain");
    }
    const domain = numericDomain(param);
    if (!requireDefault) return;
    if (!Array.isArray(param.default_reprs) || param.default_reprs.length !== 1) {
      fail(param, "must provide one scalar host-control default");
    }
    const defaultValue = scalarMetadataValue(
      param,
      param.default_reprs[0],
      "default",
    );
    if (domain.integerMinimum !== null) {
      if (defaultValue < domain.integerMinimum || defaultValue > domain.integerMaximum) {
        fail(param, "has a default outside its host-control range");
      }
      if (
        domain.integerStep !== null
        && (defaultValue - domain.integerMinimum) % domain.integerStep !== 0n
      ) {
        fail(param, "has a default outside its host-control step grid");
      }
      return;
    }
    if (defaultValue < domain.minimum || defaultValue > domain.maximum) {
      fail(param, "has a default outside its host-control range");
    }
    if (domain.step !== null) {
      const tolerance = param.scalar === "f32" ? 1.0e-5 : 1.0e-10;
      const index = (defaultValue - domain.minimum) / domain.step;
      if (
        !Number.isFinite(index)
        || Math.abs(index - Math.round(index))
          > tolerance * Math.max(Math.abs(index), 1)
      ) {
        fail(param, "has a default outside its host-control step grid");
      }
    }
  }

  function constrainDomainPlain({ minimum, maximum, step }, plain) {
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

  function curveNormalizedToUnit(curve, normalized) {
    if (Math.abs(curve) < CURVE_LINEAR_EPSILON) return normalized;
    if (curve > 0) return 1 - curveNormalizedToUnit(-curve, 1 - normalized);
    return Math.expm1(curve * normalized) / Math.expm1(curve);
  }

  function curveUnitToNormalized(curve, unit) {
    if (Math.abs(curve) < CURVE_LINEAR_EPSILON) return unit;
    if (curve > 0) return 1 - curveUnitToNormalized(-curve, 1 - unit);
    return Math.log1p(unit * Math.expm1(curve)) / curve;
  }

  function domainNormalizedToPlain(domain, normalized) {
    const {
      minimum,
      maximum,
      scale,
      curve,
    } = domain;
    const numeric = Number(normalized);
    const unit = Number.isNaN(numeric)
      ? 0
      : Math.min(1, Math.max(0, numeric));
    if (unit === 0) return minimum;
    if (unit === 1) return maximum;
    let plain;
    if (curve !== null) {
      plain = minimum + curveNormalizedToUnit(curve, unit) * (maximum - minimum);
    } else if (scale === SCALES[1]) {
      const logMinimum = Math.log(minimum);
      plain = Math.exp(logMinimum + unit * (Math.log(maximum) - logMinimum));
    } else {
      plain = minimum + unit * (maximum - minimum);
    }
    return constrainDomainPlain(domain, plain);
  }

  function domainPlainToNormalized(domain, plain) {
    const {
      minimum,
      maximum,
      scale,
      curve,
    } = domain;
    const constrained = constrainDomainPlain(domain, plain);
    if (constrained === minimum) return 0;
    if (constrained === maximum) return 1;
    let normalized;
    if (curve !== null) {
      normalized = curveUnitToNormalized(
        curve,
        (constrained - minimum) / (maximum - minimum),
      );
    } else if (scale === SCALES[1]) {
      const logMinimum = Math.log(minimum);
      normalized = (Math.log(constrained) - logMinimum)
        / (Math.log(maximum) - logMinimum);
    } else {
      normalized = (constrained - minimum) / (maximum - minimum);
    }
    return Math.min(1, Math.max(0, normalized));
  }

  function createParamControl(param) {
    const scalar = scalarKind(param);
    if (scalar === "bool") {
      return Object.freeze({
        name: typeof param.name === "string" ? param.name : null,
        scalar,
        minimum: null,
        maximum: null,
        scale: null,
        curve: null,
        unit: null,
        step: null,
        stepCount: null,
        constrainPlain: (plain) => Boolean(plain),
        normalizedToPlain: (normalized) => Number(normalized) >= 0.5,
        plainToNormalized: (plain) => (Boolean(plain) ? 1 : 0),
      });
    }

    const domain = numericDomain(param);
    return Object.freeze({
      name: typeof param.name === "string" ? param.name : null,
      scalar,
      minimum: domain.minimum,
      maximum: domain.maximum,
      scale: domain.scale,
      curve: domain.curve,
      unit: param.param_control.unit,
      step: domain.step,
      stepCount: param.param_control.step_count,
      constrainPlain: (plain) => constrainDomainPlain(domain, plain),
      normalizedToPlain: (normalized) => domainNormalizedToPlain(domain, normalized),
      plainToNormalized: (plain) => domainPlainToNormalized(domain, plain),
    });
  }

  function constrainParamPlain(param, plain) {
    return createParamControl(param).constrainPlain(plain);
  }

  function paramNormalizedToPlain(param, normalized) {
    return createParamControl(param).normalizedToPlain(normalized);
  }

  function paramPlainToNormalized(param, plain) {
    return createParamControl(param).plainToNormalized(plain);
  }

  Object.defineProperty(globalThis, GLOBAL_NAME, {
    configurable: false,
    enumerable: false,
    writable: false,
    value: Object.freeze({
      scales: SCALES,
      validateParamControlDomain,
      createParamControl,
      constrainParamPlain,
      paramNormalizedToPlain,
      paramPlainToNormalized,
    }),
  });
})();
