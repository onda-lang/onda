// Numeric worklet failures. Only the main-thread adapter formats these as errors.
export const ONDA_RUNTIME_ERROR = Object.freeze({
  UNINITIALIZED: -1,
  INVALID_INIT_MODE: -2,
  UNKNOWN_EVENT: -3,
  INVALID_PAYLOAD: -4,
  PAYLOAD_CAPACITY: -5,
  INVALID_SNAPSHOT: -6,
  UNKNOWN_OPERATION: -7,
  INVALID_PARAMETER: -8,
  INVALID_SMOOTHING: -9,
  INVALID_AUDIO_SHAPE: -10,
  INVALID_OUTPUT: -11,
  UNEXPECTED_FAILURE: -12,
  TRANSPORT_OVERFLOW: -13,
  INVALID_STORAGE: -14,
  UNKNOWN_BUFFER: -15,
});

export const RUNTIME_ERROR_MESSAGES = Object.freeze({
  [-1]: "full initialization is required",
  [-2]: "invalid Onda init mode",
  [-3]: "unknown Onda event",
  [-4]: "event payload must be encoded bytes",
  [-5]: "event payload exceeds configured capacity",
  [-6]: "snapshot storage has an invalid type or size",
  [-7]: "unknown Onda worklet operation",
  [-8]: "invalid parameter selector, element, value, or storage",
  [-9]: "invalid parameter smoothing duration",
  [-10]: "AudioWorklet channels have missing or inconsistent frame counts",
  [-11]: "processor returned invalid execution-output storage",
  [-12]: "unexpected processor failure; full initialization is required",
  [-13]: "execution-status transport overflowed; recreate the processor",
  [-14]: "invalid processor storage",
  [-15]: "unknown Onda buffer",
});

export const STATUS_OPERATIONS = Object.freeze([
  "unknown", "process", "init", "event", "set-param", "set-param-smoothing",
  "reset-params", "restore-snapshot", "snapshot", "read-control-outputs",
  "read-buffer", "delegate-subscription", "print-subscription",
]);

export function failureStatus(error) {
  return Number.isInteger(error) && (error > 0 || RUNTIME_ERROR_MESSAGES[error])
    ? error
    : ONDA_RUNTIME_ERROR.UNEXPECTED_FAILURE;
}
