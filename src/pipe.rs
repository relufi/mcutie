use core::future::poll_fn;
use core::{cell::RefCell, task::Poll};
use embassy_sync::blocking_mutex::{raw::RawMutex, Mutex};
use embassy_sync::waitqueue::WakerRegistration;

pub(crate) trait Clean {
    fn clean(&mut self);
}

struct PipeData<T: Clean> {
    connect_count: usize,
    receiver_waker: WakerRegistration,
    sender_waker: WakerRegistration,
    pending: T,
    state: State,
}

#[derive(Clone, Copy)]
enum State {
    ReadStart,
    ReadEnd,
    WriteStart,
    WriteEnd,
}

pub(crate) struct PipeReader<'a, M: RawMutex, T: Clean> {
    pipe: &'a ConnectedPipe<M, T>,
    data: &'a T,
}
impl<M: RawMutex, T: Clean> PipeReader<'_, M, T> {
    pub(crate) fn read(&self) -> &T {
        self.data
    }
}
impl<M: RawMutex, T: Clean> Drop for PipeReader<'_, M, T> {
    fn drop(&mut self) {
        self.pipe.inner.lock(|cell| {
            let mut inner = cell.borrow_mut();
            inner.state = State::WriteEnd;
            inner.sender_waker.wake();
        })
    }
}

pub(crate) struct ConnectReader<'a, M: RawMutex, T: Clean> {
    pipe: &'a ConnectedPipe<M, T>,
}

impl<M: RawMutex, T: Clean> ConnectReader<'_, M, T> {
    pub(crate) async fn receive(&self) -> PipeReader<'_, M, T> {
        poll_fn(|cx| {
            self.pipe.inner.lock(|s| {
                let s = &mut *s.borrow_mut();
                if let State::ReadEnd = s.state {
                    s.state = State::WriteStart;
                    // s.receiver_waker.wake();
                    Poll::Ready(unsafe{
                        PipeReader {
                            pipe: &*(self.pipe as *const ConnectedPipe<M, T>),
                            data: &*(&s.pending as *const T),
                        }
                    })
                } else {
                    s.receiver_waker.register(cx.waker());
                    Poll::Pending
                }
            })
        })
        .await
    }
}

impl<M: RawMutex, T: Clean> Drop for ConnectReader<'_, M, T> {
    fn drop(&mut self) {
        self.pipe.inner.lock(|cell| {
            let mut inner = cell.borrow_mut();
            inner.connect_count -= 1;

            if inner.connect_count == 0 {
                inner.state = State::WriteEnd;
            }
            inner.sender_waker.wake();
        })
    }
}

/// A pipe that knows whether a receiver is connected. If so pushing to the
/// queue waits until there is space in the queue, otherwise data is simply
/// dropped.
pub(crate) struct ConnectedPipe<M: RawMutex, T: Clean> {
    inner: Mutex<M, RefCell<PipeData<T>>>,
}

impl<M: RawMutex, T: Clean> ConnectedPipe<M, T> {
    pub(crate) const fn new(data: T) -> Self {
        Self {
            inner: Mutex::new(RefCell::new(PipeData {
                connect_count: 0,
                receiver_waker: WakerRegistration::new(),
                sender_waker: WakerRegistration::new(),
                pending: data,
                state: State::WriteEnd,
            })),
        }
    }

    /// A future that waits for a new item to be available.
    pub(crate) fn reader(&self) -> ConnectReader<'_, M, T> {
        self.inner.lock(|cell| {
            let mut inner = cell.borrow_mut();
            inner.connect_count += 1;

            ConnectReader { pipe: self }
        })
    }

    pub(crate) async fn writer<'a>(&'a self) -> PipeWriter<'a, M, T> {
        poll_fn(|cx| {
            self.inner.lock(|s| {
                let s = &mut *s.borrow_mut();
                if let State::WriteEnd = s.state {
                    s.state = State::ReadStart;
                    s.pending.clean();
                    // s.sender_waker.wake();
                    Poll::Ready(unsafe{
                        PipeWriter {
                            pipe: &*(self as *const ConnectedPipe<M, T>),
                            data: &mut *(&mut s.pending as *mut T),
                            commit: true,
                        }
                    })
                } else {
                    s.sender_waker.register(cx.waker());
                    Poll::Pending
                }
            })
        })
        .await
    }
}

pub(crate) struct PipeWriter<'a, M: RawMutex, T: Clean> {
    pipe: &'a ConnectedPipe<M, T>,
    data: &'a mut T,
    commit: bool,
}
impl<M: RawMutex, T: Clean> PipeWriter<'_, M, T> {
    pub(crate) fn write(&mut self) -> &mut T {
        self.data
    }

    pub fn rollback(&mut self) {
        self.commit = false;
    }
    pub fn commit(&mut self) {
        self.commit = true;
    }
}
impl<M: RawMutex, T: Clean> Drop for PipeWriter<'_, M, T> {
    fn drop(&mut self) {
        self.pipe.inner.lock(|cell| {
            let mut inner = cell.borrow_mut();
            if self.commit {
                inner.state = State::ReadEnd;
                inner.receiver_waker.wake();
            } else {
                inner.state = State::WriteEnd;
                inner.sender_waker.wake();
            }
        })
    }
}
