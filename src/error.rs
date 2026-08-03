#[cfg(feature = "diagnostics")]
pub type Error = eyre::Error;
#[cfg(not(feature = "diagnostics"))]
pub type Error = Box<dyn std::error::Error + Send + Sync>;

pub type Result<T> = std::result::Result<T, Error>;

#[cfg(feature = "diagnostics")]
pub fn install_handler() -> Result<()> {
    color_eyre::install()
}

#[cfg(not(feature = "diagnostics"))]
pub fn install_handler() -> Result<()> {
    Ok(())
}

#[cfg(feature = "diagnostics")]
pub fn thread_join_error() -> Error {
    eyre::eyre!("Failed to join scanner thread")
}

#[cfg(not(feature = "diagnostics"))]
pub fn thread_join_error() -> Error {
    std::io::Error::other("Failed to join scanner thread").into()
}
