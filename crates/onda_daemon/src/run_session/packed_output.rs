//! Ownership transfer of preallocated processor output. Decoding belongs on a
//! consumer thread; the producer only swaps buffers and resets counters.
use super::*;

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct RunOutputCounts {
    pub delegates: u32,
    pub delegate_overflow: u32,
    pub prints: u32,
    pub print_overflow: u32,
}

impl RunOutputCounts {
    pub fn is_empty(self) -> bool {
        self == Self::default()
    }
}

pub struct RunPackedOutput {
    delegates: Vec<u8>,
    delegate_used: usize,
    prints: Vec<u8>,
    print_used: usize,
    counts: RunOutputCounts,
}

/// Immutable decoding metadata shared with the compiled processor.
pub struct RunOutputDecoder {
    jit: JitProgram,
    delegate_capacity: usize,
    print_capacity: usize,
}

impl RunOutputDecoder {
    pub fn allocate_packet(&self) -> RunPackedOutput {
        RunPackedOutput {
            delegates: vec![0; self.delegate_capacity],
            delegate_used: 0,
            prints: vec![0; self.print_capacity],
            print_used: 0,
            counts: RunOutputCounts::default(),
        }
    }

    pub fn decode(
        &self,
        packet: &mut RunPackedOutput,
    ) -> Result<(RunDelegateBatch, RunPrintBatch), Diagnostic> {
        let delegates = decode_run_delegate_batch(
            &self.jit,
            &packet.delegates[..packet.delegate_used],
            packet.counts.delegates,
            packet.counts.delegate_overflow,
        )?;
        let mut prints = unsafe {
            PrintBatch::from_raw_parts(packet.prints.as_mut_ptr(), packet.prints.len() as u32)
        };
        prints.used_bytes = packet.print_used as u32;
        prints.record_count = packet.counts.prints;
        prints.overflow_count = packet.counts.print_overflow;
        Ok((delegates, decode_run_print_batch(&self.jit, &prints)?))
    }
}

impl RunSession {
    pub fn output_decoder(&self) -> RunOutputDecoder {
        RunOutputDecoder {
            jit: self.jit.clone(),
            delegate_capacity: self.delegate_storage.len(),
            print_capacity: self.print_storage.len(),
        }
    }

    pub fn output_counts(&self) -> RunOutputCounts {
        RunOutputCounts {
            delegates: self.delegate_record_count,
            delegate_overflow: self.delegate_overflow_count,
            prints: self.print_record_count,
            print_overflow: self.print_overflow_count,
        }
    }

    pub fn discard_output(&mut self) {
        self.begin_delegate_batch();
        self.begin_print_batch();
    }

    /// The packet must have been allocated by this session's decoder.
    /// This operation neither allocates nor copies payload bytes.
    pub fn swap_output(&mut self, packet: &mut RunPackedOutput) {
        assert_eq!(self.delegate_storage.len(), packet.delegates.len());
        assert_eq!(self.print_storage.len(), packet.prints.len());
        packet.counts = self.output_counts();
        packet.delegate_used = self.delegate_used;
        packet.print_used = self.print_used;
        mem::swap(&mut self.delegate_storage, &mut packet.delegates);
        mem::swap(&mut self.print_storage, &mut packet.prints);
        self.discard_output();
    }
}
