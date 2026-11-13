use reqwest::StatusCode;

use crate::io::source_client::SourceFetchError;

pub(super) enum TransportClassification {
    NotFound(String),
    HttpResponse { url: String, status: u16 },
    Temporary,
    Terminal,
}

impl TransportClassification {
    pub(super) fn into_fetch_error(
        self,
        source: impl std::error::Error + 'static,
    ) -> SourceFetchError {
        match self {
            Self::NotFound(url) => SourceFetchError::not_found(&url),
            Self::HttpResponse { url, status } => {
                SourceFetchError::http_response(&url, status, source)
            }
            Self::Temporary => SourceFetchError::temporary(source),
            Self::Terminal => SourceFetchError(Box::new(source)),
        }
    }
}

impl SourceFetchError {
    fn classify_reqwest_ref(err: &reqwest::Error) -> TransportClassification {
        if let Some(status) = err.status() {
            let url = err
                .url()
                .map_or("<unknown>", reqwest::Url::as_str)
                .to_owned();
            if status == StatusCode::NOT_FOUND {
                return TransportClassification::NotFound(url);
            }
            return TransportClassification::HttpResponse {
                url,
                status: status.as_u16(),
            };
        }
        if err.is_timeout() || err.is_connect() || err.is_request() || err.is_body() {
            return TransportClassification::Temporary;
        }
        classify_error_chain(std::error::Error::source(err))
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

fn classify_single_error(err: &(dyn std::error::Error + 'static)) -> TransportClassification {
    if let Some(mw_err) = err.downcast_ref::<reqwest_middleware::Error>() {
        return classify_middleware(mw_err);
    }
    if let Some(req_err) = err.downcast_ref::<reqwest::Error>() {
        return SourceFetchError::classify_reqwest_ref(req_err);
    }
    if let Some(io_err) = err.downcast_ref::<std::io::Error>() {
        if is_transient_io_error(io_err) {
            return TransportClassification::Temporary;
        }
    }
    TransportClassification::Terminal
}

fn classify_error_chain(
    mut current: Option<&(dyn std::error::Error + 'static)>,
) -> TransportClassification {
    while let Some(err) = current {
        let class = classify_single_error(err);
        if !matches!(class, TransportClassification::Terminal) {
            return class;
        }
        current = err.source();
    }
    TransportClassification::Terminal
}

fn classify_middleware(err: &reqwest_middleware::Error) -> TransportClassification {
    match err {
        reqwest_middleware::Error::Reqwest(req_err) => {
            SourceFetchError::classify_reqwest_ref(req_err)
        }
        reqwest_middleware::Error::Middleware(inner) => {
            if let Some(boxed) = inner.downcast_ref::<Box<dyn std::error::Error + Send + Sync>>() {
                let class = classify_error_chain(Some(boxed.as_ref()));
                if !matches!(class, TransportClassification::Terminal) {
                    return class;
                }
            }
            for cause in inner.chain() {
                let class = classify_single_error(cause);
                if !matches!(class, TransportClassification::Terminal) {
                    return class;
                }
            }
            classify_error_chain(std::error::Error::source(err))
        }
    }
}

impl From<reqwest::Error> for SourceFetchError {
    fn from(err: reqwest::Error) -> Self {
        SourceFetchError::classify_reqwest_ref(&err).into_fetch_error(err)
    }
}

impl From<reqwest_middleware::Error> for SourceFetchError {
    fn from(err: reqwest_middleware::Error) -> Self {
        match err {
            reqwest_middleware::Error::Reqwest(error) => error.into(),
            other => classify_middleware(&other).into_fetch_error(other),
        }
    }
}
