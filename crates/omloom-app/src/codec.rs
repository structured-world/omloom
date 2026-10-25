use bytes::Buf;
use omloom_contracts::{Limits, MemoryBudget, WorkspaceRequest, workspace::Reply};
use tokio::sync::{Semaphore, SemaphorePermit};
use tonic::{
    Code, Status,
    codec::{Codec, DecodeBuf, Decoder},
};

// Slots are acquired before prost allocates and live through application work.
// Each frame has a separately enforced 16 MiB conservative allocation envelope.
static FRAMES: Semaphore = Semaphore::const_new(8);
pub struct Received {
    pub(crate) admitted: WorkspaceRequest,
    _slot: SemaphorePermit<'static>,
}
pub struct WorkspaceCodec;
impl Default for WorkspaceCodec {
    fn default() -> Self {
        Self
    }
}
impl Codec for WorkspaceCodec {
    type Encode = Reply;
    type Decode = Received;
    type Encoder = tonic_prost::ProstEncoder<Reply>;
    type Decoder = RequestDecoder;
    fn encoder(&mut self) -> Self::Encoder {
        tonic_prost::ProstEncoder::new(Default::default())
    }
    fn decoder(&mut self) -> Self::Decoder {
        RequestDecoder
    }
}
pub struct RequestDecoder;
impl Decoder for RequestDecoder {
    type Item = Received;
    type Error = Status;
    fn decode(&mut self, buffer: &mut DecodeBuf<'_>) -> Result<Option<Received>, Status> {
        let slot = FRAMES.try_acquire().map_err(|_| {
            super::status(
                Code::ResourceExhausted,
                "BUSY",
                "The local host is busy. Retry this request.",
            )
        })?;
        let limits = Limits {
            bytes: super::FRAME_LIMIT,
            ..Default::default()
        };
        let budget = MemoryBudget::new(16 * 1_048_576);
        let admitted = omloom_contracts::decode_workspace(buffer.chunk(), limits, &budget)
            .map_err(|_| {
                super::status(
                    Code::InvalidArgument,
                    "INVALID_FRAME",
                    "The application frame is malformed or exceeds its fixed resource bounds.",
                )
            })?;
        buffer.advance(buffer.remaining());
        Ok(Some(Received {
            admitted,
            _slot: slot,
        }))
    }
}
