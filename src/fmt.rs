#![macro_use]

use core::fmt;

pub(crate) struct Debug2Format<D: fmt::Debug>(pub(crate) D);

impl<D: fmt::Debug> fmt::Debug for Debug2Format<D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}


