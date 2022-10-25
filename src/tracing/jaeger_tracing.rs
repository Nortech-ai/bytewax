use opentelemetry::runtime::Tokio;
use pyo3::{exceptions::PyValueError, pyclass, pymethods, PyAny, PyResult};
use tracing_subscriber::{layer::SubscriberExt, EnvFilter, Layer, Registry};

use super::{log_layer, TracerBuilder, TracingConfig, TracingSetupError};

/// Configure tracing to send traces to a Jaeger instance.
///
/// The endpoint can be configured with the parameter passed to this config,
/// or with two environment variables:
///
///   OTEL_EXPORTER_JAEGER_AGENT_HOST="127.0.0.1"
///   OTEL_EXPORTER_JAEGER_AGENT_PORT="6831"
///
/// By default the endpoint is set to "127.0.0.1:6831".
///
/// If the environment variables are set, the endpoint is changed to that.
///
/// If a config option is passed to JaegerConfig,
/// it takes precedence over env vars.
#[pyclass(module="bytewax.tracing", extends=TracingConfig)]
#[pyo3(text_signature = "(service_name, endpoint)")]
#[derive(Clone)]
pub(crate) struct JaegerConfig {
    /// Service name, identifies this dataflow.
    service_name: String,
    /// Optional Jaeger's URL
    endpoint: Option<String>,
}

impl TracerBuilder for JaegerConfig {
    fn setup(&self) -> Result<(), TracingSetupError> {
        opentelemetry::global::set_text_map_propagator(opentelemetry_jaeger::Propagator::new());
        let mut tracer =
            opentelemetry_jaeger::new_agent_pipeline().with_service_name(self.service_name.clone());

        // Overwrite the endpoint if set here
        if let Some(endpoint) = self.endpoint.as_ref() {
            tracer = tracer.with_endpoint(endpoint);
        }

        let tracer = tracer
            .install_batch(Tokio)
            .map_err(|err| TracingSetupError::InitRuntime(err.to_string()))?;

        let layer = tracing_opentelemetry::layer()
            .with_tracer(tracer)
            .with_filter(EnvFilter::new("bytewax=trace,error"));
        let subscriber = Registry::default()
            .with(layer)
            // Add logs too
            .with(log_layer());
        tracing::subscriber::set_global_default(subscriber)
            .map_err(|err| TracingSetupError::Init(err.to_string()))
    }
}

#[pymethods]
impl JaegerConfig {
    #[new]
    #[args(service_name, endpoint)]
    pub(crate) fn py_new(service_name: String, endpoint: Option<String>) -> (Self, TracingConfig) {
        (
            Self {
                service_name,
                endpoint,
            },
            TracingConfig {},
        )
    }

    /// Pickle as a tuple.
    fn __getstate__(&self) -> (&str, String, Option<String>) {
        (
            "JaegerConfig",
            self.service_name.clone(),
            self.endpoint.clone(),
        )
    }

    /// Egregious hack see [`SqliteRecoveryConfig::__getnewargs__`].
    fn __getnewargs__(&self) -> (String, Option<String>) {
        (String::new(), None)
    }

    /// Unpickle from tuple of arguments.
    fn __setstate__(&mut self, state: &PyAny) -> PyResult<()> {
        if let Ok(("JaegerConfig", service_name, endpoint)) = state.extract() {
            self.service_name = service_name;
            self.endpoint = endpoint;
            Ok(())
        } else {
            Err(PyValueError::new_err(format!(
                "bad pickle contents for JaegerConfig: {state:?}"
            )))
        }
    }
}
