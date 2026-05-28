use super::*;

#[test]
fn format_bytes_zero() {
    assert_eq!(format_bytes(0), "0 B");
}

#[test]
fn format_bytes_bytes_exact() {
    assert_eq!(format_bytes(1), "1 B");
    assert_eq!(format_bytes(1023), "1023 B");
}

#[test]
fn format_bytes_kib() {
    assert_eq!(format_bytes(1024), "1.00 KiB");
    assert_eq!(format_bytes(2048), "2.00 KiB");
    assert_eq!(format_bytes(1536), "1.50 KiB");
}

#[test]
fn format_bytes_mib() {
    assert_eq!(format_bytes(1048576), "1.00 MiB");
    assert_eq!(format_bytes(2097152), "2.00 MiB");
}

#[test]
fn format_bytes_gib() {
    assert_eq!(format_bytes(1073741824), "1.00 GiB");
}

#[test]
fn format_bytes_tib() {
    let tib = 1099511627776u64;
    assert_eq!(format_bytes(tib), "1.00 TiB");
}

#[test]
fn ratio_pct_zero_denominator() {
    assert_eq!(ratio_pct(100, 0), 0.0);
}

#[test]
fn ratio_pct_half() {
    assert!((ratio_pct(50, 100) - 50.0).abs() < f64::EPSILON);
}

#[test]
fn ratio_pct_full() {
    assert!((ratio_pct(100, 100) - 100.0).abs() < f64::EPSILON);
}

#[test]
fn ratio_pct_zero() {
    assert!((ratio_pct(0, 100) - 0.0).abs() < f64::EPSILON);
}
