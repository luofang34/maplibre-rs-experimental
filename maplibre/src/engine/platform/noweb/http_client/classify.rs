use reqwest::StatusCode;

use crate::io::source_client::SourceFetchError;

pub(super) enum TransportClassification {
    NotFound(String),
    HttpResponse { url: String, status: u16 },
    Temporary,
    Terminal,
}

impl TransportClassification {
    fn from_reqwest(err: &reqwest::Error) -> Self {
        if let Some(status) = err.status() {
            let url = err
                .url()
                .map_or("<unknown>", reqwest::Url::as_str)
                .to_owned();
            if status == StatusCode::NOT_FOUND {
                return Self::NotFound(url);
            }
            return Self::HttpResponse {
                url,
                status: status.as_u16(),
            };
        }
        if err.is_timeout() || err.is_connect() || err.is_request() || err.is_body() {
            return Self::Temporary;
        }
        Self::from_error_chain(std::error::Error::source(err))
    }

    fn from_single_error(err: &(dyn std::error::Error + 'static)) -> Self {
        if let Some(mw_err) = err.downcast_ref::<reqwest_middleware::Error>() {
            return Self::from_middleware(mw_err);
        }
        if let Some(req_err) = err.downcast_ref::<reqwest::Error>() {
            return Self::from_reqwest(req_err);
        }
        if let Some(io_err) = err.downcast_ref::<std::io::Error>() {
            if is_transient_io_error(io_err) {
                return Self::Temporary;
            }
        }
        Self::Terminal
    }

    fn from_error_chain(mut current: Option<&(dyn std::error::Error + 'static)>) -> Self {
        while let Some(err) = current {
            let class = Self::from_single_error(err);
            if !matches!(class, Self::Terminal) {
                return class;
            }
            current = err.source();
        }
        Self::Terminal
    }

    fn from_middleware(err: &reqwest_middleware::Error) -> Self {
        match err {
            reqwest_middleware::Error::Reqwest(req_err) => Self::from_reqwest(req_err),
            reqwest_middleware::Error::Middleware(inner) => {
                if let Some(boxed) =
                    inner.downcast_ref::<Box<dyn std::error::Error + Send + Sync>>()
                {
                    let class = Self::from_error_chain(Some(boxed.as_ref()));
                    if !matches!(class, Self::Terminal) {
                        return class;
                    }
                }
                for cause in inner.chain() {
                    let class = Self::from_single_error(cause);
                    if !matches!(class, Self::Terminal) {
                        return class;
                    }
                }
                Self::from_error_chain(std::error::Error::source(err))
            }
        }
    }
}

fn is_transient_io_error(err: &std::io::Error) -> bool {
    use std::io::ErrorKind;
    match err.kind() {
        ErrorKind::TimedOut
        | ErrorKind::ConnectionRefused
        | ErrorKind::ConnectionReset
        | ErrorKind::ConnectionAborted
        | ErrorKind::NotConnected
        | ErrorKind::AddrNotAvailable
        | ErrorKind::NetworkUnreachable
        | ErrorKind::HostUnreachable
        | ErrorKind::Interrupted
        | ErrorKind::WouldBlock
        | ErrorKind::UnexpectedEof => true,
        ErrorKind::Other => {
            let msg = err.to_string().to_ascii_lowercase();
            msg.contains("dns")
                || msg.contains("lookup address")
                || msg.contains("name resolution")
                || msg.contains("nodename nor servname")
                || msg.contains("name or service not known")
                || msg.contains("no address associated")
        }
        _ => false,
    }
}

impl SourceFetchError {
    fn from_classification(
        class: TransportClassification,
        source: impl std::error::Error + 'static,
    ) -> Self {
        match class {
            TransportClassification::NotFound(url) => Self::not_found(&url),
            TransportClassification::HttpResponse { url, status } => {
                Self::http_response(&url, status, source)
            }
            TransportClassification::Temporary => Self::temporary(source),
            TransportClassification::Terminal => Self(Box::new(source)),
        }
    }
}

impl From<reqwest::Error> for SourceFetchError {
    fn from(err: reqwest::Error) -> Self {
        Self::from_classification(TransportClassification::from_reqwest(&err), err)
    }
}

impl From<reqwest_middleware::Error> for SourceFetchError {
    fn from(err: reqwest_middleware::Error) -> Self {
        match err {
            reqwest_middleware::Error::Reqwest(error) => error.into(),
            other => {
                Self::from_classification(TransportClassification::from_middleware(&other), other)
            }
        }
    }
}
