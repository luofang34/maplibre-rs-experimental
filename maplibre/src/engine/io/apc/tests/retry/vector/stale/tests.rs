#[tokio::test]
async fn old_successful_payloads_do_not_attach_to_a_reused_coordinate() {
    super::stale_success(true).await;
}

#[tokio::test]
async fn payloads_of_a_direct_request_cannot_override_a_later_attempt() {
    super::stale_success(false).await;
}
