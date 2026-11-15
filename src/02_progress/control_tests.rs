use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

use super::CancellationHook;
use super::OperationControl;

#[derive(Debug)]
struct TestCancellation(AtomicBool);

impl CancellationHook for TestCancellation {
    fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

#[test]
fn operation_control_returns_typed_cancellation() {
    let cancellation = Arc::new(TestCancellation(AtomicBool::new(false)));
    let control = OperationControl::default().with_cancellation(cancellation.clone());

    assert!(control.checkpoint().is_ok());
    cancellation.0.store(true, Ordering::Release);
    assert!(matches!(control.checkpoint(), Err(crate::Error::Cancelled)));
}
