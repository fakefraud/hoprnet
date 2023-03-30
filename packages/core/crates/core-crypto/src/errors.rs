use thiserror::Error;
use utils_types::errors::GeneralError;

#[derive(Error, Debug)]
pub enum CryptoError {
    #[error("cryptographic parameter '{name:?}' must be {expected:?} bytes")]
    InvalidParameterSize { name: String, expected: usize },

    #[error("input to the function has invalid value or size")]
    InvalidInputValue,

    #[error("secret scalar results in an invalid EC point")]
    InvalidSecretScalar,

    #[error("elliptic curve error: {0}")]
    EllipticCurveError(#[from] elliptic_curve::Error),

    #[error("failed to perform cryptographic calculation")]
    CalculationError,

    #[error("lower-level error: {0}")]
    Other(#[from] GeneralError),
}

pub type Result<T> = core::result::Result<T, CryptoError>;