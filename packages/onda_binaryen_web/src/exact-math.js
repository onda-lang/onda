// Versioned bit-level math support for operations that core WebAssembly does
// not provide. The ABI deliberately passes integer bit patterns, rather than
// JavaScript Numbers, so NaN payload conversion and an intermediate floating-
// point rounding cannot affect the result.
export const ONDA_EXACT_MATH_ABI_VERSION = 1;
export const ONDA_EXACT_MATH_IMPORT_MODULE =
  `onda_exact_math_v${ONDA_EXACT_MATH_ABI_VERSION}`;

const EXACT_F32 = makeExactFloatFormat(32, 8, 23, 127);
const EXACT_F64 = makeExactFloatFormat(64, 11, 52, 1023);

function makeExactFloatFormat(totalBits, exponentBits, fractionBits, bias) {
  const width = BigInt(totalBits);
  const fractionWidth = BigInt(fractionBits);
  const exponentFieldMask = (1n << BigInt(exponentBits)) - 1n;
  const fractionMask = (1n << fractionWidth) - 1n;
  const hiddenBit = 1n << fractionWidth;
  return Object.freeze({
    totalBits,
    fractionBits,
    bias,
    minExponent: 1 - bias,
    maxExponent: bias,
    signMask: 1n << (width - 1n),
    exponentFieldMask,
    fractionMask,
    hiddenBit,
    infinityBits: exponentFieldMask << fractionWidth,
    canonicalNaNBits:
      (exponentFieldMask << fractionWidth) | (1n << (fractionWidth - 1n)),
  });
}

function normalizeExactBits(bits, width) {
  return BigInt.asUintN(width, typeof bits === "bigint" ? bits : BigInt(bits));
}

function decodeExactFloat(bits, format) {
  const normalized = normalizeExactBits(bits, format.totalBits);
  const sign = (normalized & format.signMask) !== 0n;
  const exponentField = Number(
    (normalized >> BigInt(format.fractionBits)) & format.exponentFieldMask,
  );
  const fraction = normalized & format.fractionMask;
  const maximumExponentField = Number(format.exponentFieldMask);

  if (exponentField === maximumExponentField) {
    return { kind: fraction === 0n ? "infinity" : "nan", sign };
  }
  if (exponentField === 0) {
    return {
      kind: fraction === 0n ? "zero" : "finite",
      sign,
      significand: fraction,
      exponent: format.minExponent - format.fractionBits,
    };
  }
  return {
    kind: "finite",
    sign,
    significand: format.hiddenBit | fraction,
    exponent: exponentField - format.bias - format.fractionBits,
  };
}

function exactSignedBits(sign, magnitudeBits, format) {
  return (sign ? format.signMask : 0n) | magnitudeBits;
}

function roundExactShiftRightEven(value, shift) {
  if (shift <= 0) return value << BigInt(-shift);
  const width = BigInt(shift);
  const quotient = value >> width;
  const remainder = value - (quotient << width);
  const halfway = 1n << (width - 1n);
  if (remainder > halfway || (remainder === halfway && (quotient & 1n) !== 0n)) {
    return quotient + 1n;
  }
  return quotient;
}

function exactBitLength(value) {
  return value.toString(2).length;
}

// Rounds the exact non-zero value (-1)^sign * magnitude * 2^exponent to the
// destination format using IEEE-754 roundTiesToEven.
function roundExactFinite(sign, magnitude, exponent, format) {
  const magnitudeBits = exactBitLength(magnitude);
  const leadingExponent = magnitudeBits - 1 + exponent;

  if (leadingExponent < format.minExponent) {
    const subnormalUnitExponent = format.minExponent - format.fractionBits;
    const shift = subnormalUnitExponent - exponent;
    const fraction = roundExactShiftRightEven(magnitude, shift);
    if (fraction === 0n) return exactSignedBits(sign, 0n, format);
    if (fraction >= format.hiddenBit) {
      // Rounding the largest subnormal can produce the smallest normal.
      return exactSignedBits(sign, 1n << BigInt(format.fractionBits), format);
    }
    return exactSignedBits(sign, fraction, format);
  }

  if (leadingExponent > format.maxExponent) {
    return exactSignedBits(sign, format.infinityBits, format);
  }

  let roundedExponent = leadingExponent;
  const significandShift = magnitudeBits - 1 - format.fractionBits;
  let significand = roundExactShiftRightEven(magnitude, significandShift);
  if (significand === format.hiddenBit << 1n) {
    significand >>= 1n;
    roundedExponent += 1;
  }
  if (roundedExponent > format.maxExponent) {
    return exactSignedBits(sign, format.infinityBits, format);
  }

  const exponentField = BigInt(roundedExponent + format.bias);
  const fraction = significand - format.hiddenBit;
  return exactSignedBits(
    sign,
    (exponentField << BigInt(format.fractionBits)) | fraction,
    format,
  );
}

// Every finite input is decoded as an integer significand times a power of
// two. BigInt multiplication and alignment at the smaller exponent preserve
// the complete product and addend, so roundExactFinite is the only operation
// that discards bits.
function exactFmaBits(aBits, bBits, cBits, format) {
  const a = decodeExactFloat(aBits, format);
  const b = decodeExactFloat(bBits, format);
  const c = decodeExactFloat(cBits, format);

  if (a.kind === "nan" || b.kind === "nan" || c.kind === "nan") {
    return format.canonicalNaNBits;
  }

  const productSign = a.sign !== b.sign;
  const productIsInfinite = a.kind === "infinity" || b.kind === "infinity";
  if (productIsInfinite) {
    if (a.kind === "zero" || b.kind === "zero") {
      return format.canonicalNaNBits;
    }
    if (c.kind === "infinity" && c.sign !== productSign) {
      return format.canonicalNaNBits;
    }
    return exactSignedBits(productSign, format.infinityBits, format);
  }
  if (c.kind === "infinity") {
    return exactSignedBits(c.sign, format.infinityBits, format);
  }

  const productMagnitude = a.significand * b.significand;
  const productExponent = a.exponent + b.exponent;
  const addendMagnitude = c.significand;

  if (productMagnitude === 0n && addendMagnitude === 0n) {
    // Under roundTiesToEven, only -0 + -0 produces -0. Opposite signed zeros
    // and exact cancellation of non-zero values produce +0.
    return exactSignedBits(productSign && c.sign, 0n, format);
  }
  if (productMagnitude === 0n) {
    return roundExactFinite(c.sign, addendMagnitude, c.exponent, format);
  }
  if (addendMagnitude === 0n) {
    return roundExactFinite(productSign, productMagnitude, productExponent, format);
  }

  const commonExponent = Math.min(productExponent, c.exponent);
  const product = productMagnitude << BigInt(productExponent - commonExponent);
  const addend = addendMagnitude << BigInt(c.exponent - commonExponent);
  const signedProduct = productSign ? -product : product;
  const signedAddend = c.sign ? -addend : addend;
  const sum = signedProduct + signedAddend;
  if (sum === 0n) return 0n;

  return roundExactFinite(
    sum < 0n,
    sum < 0n ? -sum : sum,
    commonExponent,
    format,
  );
}

export function exactFmaF32Bits(aBits, bBits, cBits) {
  return Number(exactFmaBits(aBits, bBits, cBits, EXACT_F32));
}

export function exactFmaF64Bits(aBits, bBits, cBits) {
  return exactFmaBits(aBits, bBits, cBits, EXACT_F64);
}

export function createExactMathImports() {
  return {
    [ONDA_EXACT_MATH_IMPORT_MODULE]: {
      fma_f32_bits: (a, b, c) =>
        Number(BigInt.asIntN(32, BigInt(exactFmaF32Bits(a, b, c)))),
      fma_f64_bits: (a, b, c) =>
        BigInt.asIntN(64, exactFmaF64Bits(a, b, c)),
    },
  };
}
