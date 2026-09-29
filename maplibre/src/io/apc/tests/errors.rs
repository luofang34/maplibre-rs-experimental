#![allow(clippy::expect_used, clippy::panic)]

use std::error::Error;

use super::super::{CallError, ProcedureError, SendError};

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn scheduling_failure_keeps_the_scheduler_cause() {
    use crate::{
        environment::OffscreenKernelConfig,
        io::{
            apc::{AsyncProcedureCall, Input, SchedulerAsyncProcedureCall},
            scheduler::{NopScheduler, ScheduleError},
        },
        platform::ReqwestOffscreenKernelEnvironment,
        style::Style,
    };
    let apc = SchedulerAsyncProcedureCall::<ReqwestOffscreenKernelEnvironment, _>::new(
        NopScheduler,
        OffscreenKernelConfig {
            cache_directory: None,
        },
    );
    let error = apc
        .call(
            Input::TileRequest {
                coords: Default::default(),
                style: Style::default(),
            },
            |_, _, _| Box::pin(async { Ok(()) }),
        )
        .expect_err("NopScheduler rejects the call");
    let cause = error.source().expect("scheduler cause");
    assert!(matches!(
        cause.downcast_ref::<ScheduleError>(),
        Some(ScheduleError::NotImplemented)
    ));
}

#[test]
fn procedure_failures_keep_their_underlying_causes() {
    let execution = ProcedureError::Execution(Box::new(std::io::Error::other("decode failed")));
    assert_eq!(
        execution.source().expect("decode cause").to_string(),
        "decode failed"
    );
    let send = ProcedureError::Send(SendError::Transmission {
        operation: "sending a result",
        source: Box::new(std::io::Error::other("receiver disconnected")),
    });
    assert!(send.source().expect("send cause").is::<SendError>());
}

#[test]
fn serialized_call_failures_keep_their_underlying_causes() {
    for error in [
        CallError::Serialize(Box::new(std::io::Error::other("encode failed"))),
        CallError::Deserialize(Box::new(std::io::Error::other("decode failed"))),
        CallError::DeserializeInput(Box::new(std::io::Error::other("input failed"))),
    ] {
        assert!(error.source().expect("codec cause").is::<std::io::Error>());
    }
}
