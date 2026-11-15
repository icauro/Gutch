use super::*;

const BUDGET: u64 = 512 * 1024 * 1024;

#[test]
fn nonblocking_admission_releases_its_reservation() {
    let budget = std::sync::Arc::new(MemoryBudget::new(10));
    let permit = budget.try_acquire(7).unwrap().unwrap();
    assert!(budget.try_acquire(4).unwrap().is_none());
    assert!(budget.try_acquire(11).is_err());
    drop(permit);
    assert!(budget.try_acquire(10).unwrap().is_some());
}

#[test]
fn exhausted_memory_admission_remains_cancellable() {
    let budget = std::sync::Arc::new(MemoryBudget::new(BUDGET));
    let profiler = std::sync::Arc::new(crate::profiling::Profiler::new(
        crate::profiling::ProfileMode::Detailed,
    ));
    let control = crate::progress::OperationControl::default().with_profiler(profiler.clone());
    let permit = budget.acquire(BUDGET, &control).unwrap();
    let cancellation = std::sync::Arc::new(crate::test_util::TestCancellation::default());
    let waiting_control = control.clone().with_cancellation(cancellation.clone());
    std::thread::scope(|scope| {
        let waiter = scope.spawn(|| budget.acquire(16 * 1024 * 1024, &waiting_control));
        cancellation.cancel();
        assert!(
            waiter
                .join()
                .unwrap()
                .is_err_and(|error| error.is_cancelled())
        );
    });
    let summary = profiler.summary();
    let waits: Vec<_> = summary
        .spans
        .iter()
        .filter(|span| span.name == "runtime.memory.wait")
        .collect();
    assert_eq!(waits.len(), 1);
    assert_eq!(waits[0].count, 1);
    drop(permit);
    assert!(budget.acquire(BUDGET, &control).is_ok());
    assert!(budget.acquire(BUDGET + 1, &control).is_err());
}
