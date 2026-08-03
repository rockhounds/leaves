#[cfg(not(feature = "diagnostics"))]
pub struct NoopSpan;

#[cfg(not(feature = "diagnostics"))]
impl NoopSpan {
    pub fn in_scope<F, T>(&self, f: F) -> T
    where
        F: FnOnce() -> T,
    {
        f()
    }
}

#[cfg(feature = "diagnostics")]
macro_rules! diag_span {
    ($level:ident, $($arg:tt)*) => {
        tracing::span!(tracing::Level::$level, $($arg)*)
    };
}

#[cfg(not(feature = "diagnostics"))]
macro_rules! diag_span {
    ($($arg:tt)*) => {
        $crate::diagnostics::NoopSpan
    };
}

#[cfg(feature = "diagnostics")]
macro_rules! diag_trace {
    ($($arg:tt)*) => {
        tracing::trace!($($arg)*)
    };
}

#[cfg(not(feature = "diagnostics"))]
macro_rules! diag_trace {
    ($($arg:tt)*) => {
        ()
    };
}

#[cfg(feature = "diagnostics")]
macro_rules! diag_debug {
    ($($arg:tt)*) => {
        tracing::debug!($($arg)*)
    };
}

#[cfg(not(feature = "diagnostics"))]
macro_rules! diag_debug {
    ($($arg:tt)*) => {
        ()
    };
}

#[cfg(feature = "diagnostics")]
macro_rules! diag_info {
    ($($arg:tt)*) => {
        tracing::info!($($arg)*)
    };
}

#[cfg(not(feature = "diagnostics"))]
macro_rules! diag_info {
    ($($arg:tt)*) => {
        ()
    };
}

#[cfg(feature = "diagnostics")]
macro_rules! diag_warn {
    ($($arg:tt)*) => {
        tracing::warn!($($arg)*)
    };
}

#[cfg(not(feature = "diagnostics"))]
macro_rules! diag_warn {
    ($($arg:tt)*) => {
        ()
    };
}

#[cfg(feature = "diagnostics")]
macro_rules! diag_error {
    ($($arg:tt)*) => {
        tracing::error!($($arg)*)
    };
}

#[cfg(not(feature = "diagnostics"))]
macro_rules! diag_error {
    ($($arg:tt)*) => {
        ()
    };
}
