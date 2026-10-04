//! Tests for `obs::Sensitive` (T-201a).

use obs::Sensitive;

#[test]
fn sensitive_debug_and_display_print_redacted() {
    let s = Sensitive::new("secret-value");
    assert_eq!(format!("{s:?}"), "[redacted]");
    assert_eq!(format!("{s}"), "[redacted]");
}

#[test]
fn sensitive_expose_and_into_inner() {
    let s = Sensitive::new("value");
    assert_eq!(*s.expose(), "value");
    assert_eq!(s.into_inner(), "value");
}

#[test]
fn sensitive_clone_and_from() {
    let s = Sensitive::new("value");
    let c = s.clone();
    assert_eq!(*c.expose(), "value");
    let f: Sensitive<String> = String::from("value").into();
    assert_eq!(*f.expose(), "value");
}
