use std::sync::Arc;

use super::OperationControl;

#[test]
fn operation_control_returns_typed_cancellation() {
    let cancellation = Arc::new(crate::test_util::TestCancellation::default());
    let control = OperationControl::default().with_cancellation(cancellation.clone());

    assert!(control.checkpoint().is_ok());
    cancellation.cancel();
    assert!(matches!(control.checkpoint(), Err(crate::Error::Cancelled)));
}
