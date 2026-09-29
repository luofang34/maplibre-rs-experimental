#[tokio::test]
async fn old_successful_payloads_do_not_attach_to_a_reused_coordinate() {
    super::stale_success(true).await;
}

#[tokio::test]
async fn untracked_legacy_payloads_cannot_override_an_admitted_attempt() {
    super::stale_success(false).await;
}
