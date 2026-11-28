use super::fixture::builder_without;

fn check_missing_service(missing: &str) {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        drop(builder_without(missing).build());
    }));
    assert!(
        result.is_ok(),
        "missing {missing} must return without unwinding"
    );
}

#[test]
fn missing_window_does_not_unwind() {
    check_missing_service("window");
}
#[test]
fn missing_transport_does_not_unwind() {
    check_missing_service("apc");
}
#[test]
fn missing_scheduler_does_not_unwind() {
    check_missing_service("scheduler");
}
#[test]
fn missing_http_client_does_not_unwind() {
    check_missing_service("http");
}
