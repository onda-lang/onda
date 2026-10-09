//! Host-owned, block-level linear ramps for continuous float controls.
use super::{
    default_run_param_element, scalar_param_bytes, Diagnostic, Instance, JitProgram, PrimitiveType,
};

#[derive(Debug)]
pub(super) struct ParamSmoothing {
    samples: usize,
    inverse_samples: f64,
    addresses: Vec<Vec<usize>>,
    entries: Vec<Entry>,
    active: Vec<usize>,
    started: bool,
}

#[derive(Debug)]
struct Entry {
    index: usize,
    element: usize,
    ty: PrimitiveType,
    default: f64,
    current: f64,
    start: f64,
    target: f64,
    elapsed: usize,
    active: bool,
}

impl ParamSmoothing {
    pub(super) fn new(jit: &JitProgram, seconds: f64) -> Result<Self, Diagnostic> {
        let count = (seconds * f64::from(jit.mir().config.sample_rate)).ceil();
        if !seconds.is_finite() || seconds < 0.0 || count >= usize::MAX as f64 {
            return Err(Diagnostic::runtime("param_smoothing_seconds must be finite, non-negative, and fit the host sample counter", 0, 0));
        }
        let samples = count as usize;
        let mut out = Self {
            samples,
            inverse_samples: 1.0 / samples.max(1) as f64,
            addresses: Vec::new(),
            entries: Vec::new(),
            active: Vec::new(),
            started: false,
        };
        if samples <= jit.mir().config.block_size as usize {
            return Ok(out);
        }
        for index in 0..jit.param_count() {
            let desc = jit.param_descriptor(index).expect("declared parameter");
            let continuous = matches!(desc.elem_ty(), PrimitiveType::F32 | PrimitiveType::F64)
                && !desc
                    .param_domain()
                    .is_some_and(|domain| domain.step_count().is_some());
            let mut elements = Vec::new();
            if continuous {
                for element in 0..desc.array_len() {
                    let default = default_run_param_element(desc, element);
                    elements.push(out.entries.len());
                    out.entries.push(Entry {
                        index,
                        element,
                        ty: desc.elem_ty(),
                        default,
                        current: default,
                        start: default,
                        target: default,
                        elapsed: samples,
                        active: false,
                    });
                }
            }
            out.addresses.push(elements);
        }
        out.active = Vec::with_capacity(out.entries.len());
        Ok(out)
    }

    /// Returns whether this write is handled by a ramp. Discrete and disabled
    /// controls, and transitions involving non-finite values, are written directly.
    pub(super) fn set_target(&mut self, index: usize, element: usize, value: f64) -> bool {
        let Some(&id) = self
            .addresses
            .get(index)
            .and_then(|elements| elements.get(element))
        else {
            return false;
        };
        let entry = &mut self.entries[id];
        let target = if entry.ty == PrimitiveType::F32 {
            f64::from(value as f32)
        } else {
            value
        };
        if !self.started || !target.is_finite() || !entry.current.is_finite() {
            entry.current = target;
            entry.target = target;
            entry.elapsed = self.samples;
            return false;
        }
        if target == entry.target {
            return true;
        }
        entry.start = entry.current;
        entry.target = target;
        entry.elapsed = if target == entry.current {
            self.samples
        } else {
            0
        };
        if !entry.active {
            entry.active = true;
            self.active.push(id);
        }
        true
    }

    pub(super) fn begin_block(
        &mut self,
        instance: &mut Instance,
        frames: usize,
    ) -> Result<(), Diagnostic> {
        if frames == 0 {
            return Ok(());
        }
        self.started = true;
        let mut cursor = 0;
        while cursor < self.active.len() {
            let entry = &mut self.entries[self.active[cursor]];
            entry.elapsed += frames.min(self.samples - entry.elapsed);
            let value = if entry.elapsed == self.samples {
                entry.target
            } else {
                let weight = entry.elapsed as f64 * self.inverse_samples;
                if entry.start.is_sign_negative() != entry.target.is_sign_negative() {
                    entry.start * (1.0 - weight) + entry.target * weight
                } else if weight <= 0.5 {
                    entry.start + (entry.target - entry.start) * weight
                } else {
                    entry.target + (entry.start - entry.target) * (1.0 - weight)
                }
            };
            let bytes = scalar_param_bytes(entry.ty, value)?;
            onda_runtime::set_param_element_by_index(
                instance,
                entry.index,
                entry.element,
                bytes.as_slice(),
            )?;
            entry.current = if entry.ty == PrimitiveType::F32 {
                f64::from(value as f32)
            } else {
                value
            };
            if entry.elapsed == self.samples {
                entry.active = false;
                self.active.swap_remove(cursor);
            } else {
                cursor += 1;
            }
        }
        Ok(())
    }

    pub(super) fn settle(&mut self, defaults: bool) {
        self.active.clear();
        self.started = false;
        for entry in &mut self.entries {
            if defaults {
                entry.target = entry.default;
            }
            entry.current = entry.target;
            entry.start = entry.target;
            entry.elapsed = self.samples;
            entry.active = false;
        }
    }
}
