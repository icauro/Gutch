use super::StagingFlusher;
use crate::progress::OperationControl;

#[test]
fn finish_waits_for_every_submitted_file() {
    let temp = tempfile::tempdir().unwrap();
    let flusher = StagingFlusher::new(2, &OperationControl::default()).unwrap();
    for index in 0..8 {
        let path = temp.path().join(format!("staged-{index}"));
        std::fs::write(&path, b"bytes").unwrap();
        flusher.submit(std::fs::OpenOptions::new().write(true).open(&path).unwrap());
    }

    flusher.finish().unwrap();
}

#[test]
fn dropping_an_unfinished_flusher_joins_its_threads() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("staged");
    std::fs::write(&path, b"bytes").unwrap();
    let flusher = StagingFlusher::new(3, &OperationControl::default()).unwrap();
    flusher.submit(std::fs::OpenOptions::new().write(true).open(&path).unwrap());

    drop(flusher);

    // The staged file can be discarded once the flusher is gone.
    std::fs::remove_file(&path).unwrap();
    temp.close().unwrap();
}
