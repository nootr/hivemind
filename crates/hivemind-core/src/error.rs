pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum Error {
    #[error("payload exceeds max supported size")]
    PayloadTooLarge,

    #[error("invalid object id")]
    InvalidObjectId,

    #[error("invalid object signature")]
    InvalidObjectSignature,

    #[error("invalid object body")]
    InvalidObjectBody,

    #[error("invalid chunk id")]
    InvalidChunkId,

    #[error("canonical cbor encoding failed")]
    CborEncode,

    #[error("invalid signature bytes")]
    InvalidSignatureBytes,
}
