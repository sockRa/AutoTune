// Explicit bounded teardown seam. Library results are OS results, not CUDA codes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TeardownOperation {
    Stream,
    Buffer(u8),
    Module,
    Context,
    Library,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TeardownReturn {
    Cuda(i32),
    Library {
        released: bool,
        error_code: Option<i32>,
    },
}
/// Hooks must retain the result before taking a post-clock observation. They
/// perform no native work. Any rejection ends teardown without a successor.
pub trait TeardownObserver {
    fn before(&mut self, operation: TeardownOperation) -> Result<(), Fault>;
    fn returned(&mut self, operation: TeardownOperation, raw: TeardownReturn) -> Result<(), Fault>;
}
struct UnobservedTeardown;
impl TeardownObserver for UnobservedTeardown {
    fn before(&mut self, _: TeardownOperation) -> Result<(), Fault> {
        Ok(())
    }
    fn returned(&mut self, _: TeardownOperation, _: TeardownReturn) -> Result<(), Fault> {
        Ok(())
    }
}
impl CudaBackend {
    /// Explicit one-shot teardown after observed idle, under an original stop/
    /// cooperative window checked by the caller's observer. A success-returned
    /// release clears its token before post-clock validation. Failed outputs or
    /// uncertain completion stay retained and can never be retried by close.
    pub fn close_observed(&mut self, observer: &mut impl TeardownObserver) -> Result<(), Fault> {
        self.close_with_release(observer, |library| {
            library.close().map_err(|error| {
                std::error::Error::source(&error)
                    .and_then(|source| source.downcast_ref::<std::io::Error>())
                    .and_then(std::io::Error::raw_os_error)
            })
        })
    }
    fn close_with_release(
        &mut self,
        observer: &mut impl TeardownObserver,
        release: impl FnOnce(Library) -> Result<(), Option<i32>>,
    ) -> Result<(), Fault> {
        if !matches!(self.state, State::Ready | State::Complete) {
            return Err(invalid("cleanup without idle"));
        }
        self.state = State::Poisoned;
        macro_rules! call {
            ($operation:expr,$name:literal,$invoke:expr,$clear:expr) => {{
                let op = $operation;
                observer.before(op)?;
                let status = unsafe { $invoke };
                if status == 0 {
                    $clear;
                }
                observer.returned(op, TeardownReturn::Cuda(status))?;
                checked($name, status)?;
            }};
        }
        call!(
            TeardownOperation::Stream,
            "cuStreamDestroy_v2",
            (self.api.stream_destroy)(self.stream),
            self.stream = null_mut()
        );
        for i in 0..4 {
            call!(
                TeardownOperation::Buffer(i as u8),
                "cuMemFree_v2",
                (self.api.free)(self.buffers[i]),
                self.buffers[i] = 0
            );
        }
        call!(
            TeardownOperation::Module,
            "cuModuleUnload",
            (self.api.module_unload)(self.module),
            {
                self.module = null_mut();
                self.function = null_mut();
            }
        );
        call!(
            TeardownOperation::Context,
            "cuCtxDestroy_v2",
            (self.api.ctx_destroy)(self.context),
            self.context = null_mut()
        );
        if self._library.is_some() {
            observer.before(TeardownOperation::Library)?;
            // The consuming library close suppresses its own Drop on error.
            // Preserve the original opaque handle when it fails; never issue a
            // second FreeLibrary or claim the failed handle is safe to use.
            let library =
                unsafe { ManuallyDrop::take(self._library.as_mut().expect("retained library")) };
            let raw = library.into_raw();
            self._library = None;
            let result = release(unsafe { Library::from_raw(raw) });
            if result.is_err() {
                self._library = Some(ManuallyDrop::new(unsafe { Library::from_raw(raw) }));
            }
            observer.returned(
                TeardownOperation::Library,
                TeardownReturn::Library {
                    released: result.is_ok(),
                    error_code: result.err().flatten(),
                },
            )?;
            if result.is_err() {
                return Err(invalid("FreeLibrary"));
            }
        }
        self.state = State::Closed;
        Ok(())
    }
}
