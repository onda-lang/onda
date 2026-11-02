use super::*;

#[derive(Clone, Copy)]
pub(super) enum BufferMetadata {
    Len,
    Samples,
    Channels,
    SampleRate,
    Bound,
}

impl BufferMetadata {
    pub(super) fn from_method(method: &str) -> Option<Self> {
        use crate::builtins::*;
        Some(match method {
            ARRAY_LEN_METHOD => Self::Len,
            BUFFER_SAMPLES_METHOD => Self::Samples,
            BUFFER_CHANS_METHOD => Self::Channels,
            BUFFER_SAMPLERATE_METHOD => Self::SampleRate,
            BUFFER_BOUND_METHOD => Self::Bound,
            _ => return None,
        })
    }
}

impl FunctionLowerer<'_> {
    pub(super) fn emit_buffer_metadata(
        &mut self,
        reference: MaterializedBufferReference,
        metadata: BufferMetadata,
        block: &mut MirBlock,
        location: SourceLoc,
    ) -> LoweredValue {
        use BufferMetadata::*;
        use MaterializedBufferReference::*;
        if matches!(metadata, Len) {
            let samples = self.emit_buffer_metadata(reference, Samples, block, location.clone());
            let channels = self.emit_buffer_metadata(reference, Channels, block, location.clone());
            return self.emit_temp(
                block,
                PrimitiveType::I32,
                Rvalue::Binary {
                    op: MirBinaryOp::Multiply,
                    lhs: samples.value,
                    rhs: channels.value,
                },
                location,
            );
        }
        let ty = match metadata {
            SampleRate => PrimitiveType::F32,
            Bound => PrimitiveType::Bool,
            _ => PrimitiveType::I32,
        };
        let value = match (reference, metadata) {
            (Interface(buffer), Samples) => Rvalue::BufferSamples(buffer),
            (Interface(buffer), Channels) => Rvalue::BufferChannels(buffer),
            (Interface(buffer), SampleRate) => Rvalue::BufferSampleRate(buffer),
            (Interface(buffer), Bound) => Rvalue::BufferIsBound(buffer),
            (Parameter(buffer), Samples) => Rvalue::BufferParamSamples(buffer),
            (Parameter(buffer), Channels) => Rvalue::BufferParamChannels(buffer),
            (Parameter(buffer), SampleRate) => Rvalue::BufferParamSampleRate(buffer),
            (Parameter(buffer), Bound) => Rvalue::BufferParamIsBound(buffer),
            (_, Len) => unreachable!("element count is a product of buffer dimensions"),
        };
        self.emit_temp(block, ty, value, location)
    }
}
