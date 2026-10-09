//! Station errors.

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("radio: {0}")]
    Radio(String),
    #[error("the radio did not come online in time")]
    RadioTimeout,
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("lxmf: {0}")]
    Lxmf(String),
    #[error("the requested profile is not a valid LoRa configuration")]
    Profile,
}
