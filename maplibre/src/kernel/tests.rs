#![allow(clippy::expect_used, clippy::panic)]

use super::*;
use crate::{
    io::apc::AsyncProcedureCall,
    io::scheduler::{ScheduleError, Scheduler},
    window::{MapWindow, MapWindowConfig},
};

mod fixture;
use fixture::{builder_without, Client, TestEnvironment, WindowConfig};
mod panic_probes;

#[test]
fn omitted_dependencies_return_the_named_configuration_error() {
    for (missing, expected) in [
        ("window", KernelBuildError::MissingWindowConfig),
        ("apc", KernelBuildError::MissingAsyncProcedureCall),
        ("scheduler", KernelBuildError::MissingScheduler),
        ("http", KernelBuildError::MissingHttpClient),
    ] {
        let Err(error) = builder_without(missing).build() else {
            panic!("missing {missing} must fail");
        };
        assert_eq!(error, expected);
    }
}

#[test]
fn empty_builder_reports_the_first_missing_service() {
    assert!(matches!(
        KernelBuilder::<TestEnvironment>::new().build(),
        Err(KernelBuildError::MissingWindowConfig)
    ));
}

#[tokio::test]
async fn completed_builder_uses_the_supplied_services_and_latest_replacements() {
    let kernel = builder_without("")
        .with_map_window_config(WindowConfig(17))
        .with_http_client(Client("replacement"))
        .build()
        .expect("all services");
    assert_eq!(
        kernel
            .map_window_config()
            .create()
            .expect("window")
            .size()
            .width(),
        17
    );
    assert_eq!(
        kernel
            .source_client()
            .fetch_url("tile")
            .await
            .expect("source bytes"),
        b"replacement:tile"
    );
    assert!(matches!(
        kernel.scheduler().schedule(|| async {}),
        Err(ScheduleError::NotImplemented)
    ));
    assert!(kernel.apc().receive(|_| true).next().is_none());
}
