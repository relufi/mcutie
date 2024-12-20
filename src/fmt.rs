#![macro_use]

#[cfg(all(feature = "defmt", feature = "log"))]
compile_error!("The `defmt` and `log` features cannot both be enabled at the same time.");

#[cfg(not(feature = "defmt"))]
use core::fmt;

#[cfg(feature = "defmt")]
pub(crate) use ::defmt::Debug2Format;

#[cfg(not(feature = "defmt"))]
pub(crate) struct Debug2Format<D: fmt::Debug>(pub(crate) D);

#[cfg(feature = "log")]
impl<D: fmt::Debug> fmt::Debug for Debug2Format<D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[collapse_debuginfo(yes)]
macro_rules! trace {
    ($s:literal $(, $x:expr)* $(,)?) => {
        #[cfg(feature = "defmt")]
        ::defmt::trace!($s $(, $x)*);
        #[cfg(feature = "log")]
        ::log::trace!($s $(, $x)*);
        #[cfg(not(any(feature="defmt", feature="log")))]
        let _ = ($( & $x ),*);
    };
}

#[collapse_debuginfo(yes)]
macro_rules! debug {
    ($s:literal $(, $x:expr)* $(,)?) => {
        #[cfg(feature = "defmt")]
        ::defmt::debug!($s $(, $x)*);
        #[cfg(feature = "log")]
        ::log::debug!($s $(, $x)*);
        #[cfg(not(any(feature="defmt", feature="log")))]
        let _ = ($( & $x ),*);
    };
}

#[collapse_debuginfo(yes)]
macro_rules! info {
    ($s:literal $(, $x:expr)* $(,)?) => {
        #[cfg(feature = "defmt")]
        ::defmt::info!($s $(, $x)*);
        #[cfg(feature = "log")]
        ::log::info!($s $(, $x)*);
        #[cfg(not(any(feature="defmt", feature="log")))]
        let _ = ($( & $x ),*);
    };
}

#[collapse_debuginfo(yes)]
macro_rules! warn {
    ($s:literal $(, $x:expr)* $(,)?) => {
        #[cfg(feature = "defmt")]
        ::defmt::warn!($s $(, $x)*);
        #[cfg(feature = "log")]
        ::log::warn!($s $(, $x)*);
        #[cfg(not(any(feature="defmt", feature="log")))]
        let _ = ($( & $x ),*);
    };
}

#[collapse_debuginfo(yes)]
macro_rules! error {
    ($s:literal $(, $x:expr)* $(,)?) => {
        #[cfg(feature = "defmt")]
        ::defmt::error!($s $(, $x)*);
        #[cfg(feature = "log")]
        ::log::error!($s $(, $x)*);
        #[cfg(not(any(feature="defmt", feature="log")))]
        let _ = ($( & $x ),*);
    };
}
