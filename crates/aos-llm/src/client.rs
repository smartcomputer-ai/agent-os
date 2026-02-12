//! Core client and middleware system.
//!
//! Implemented in P05.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};

use async_trait::async_trait;
use futures::future::BoxFuture;

use crate::Response;
use crate::errors::{ConfigurationError, SDKError};
use crate::provider::{ProviderAdapter, registered_factories};
use crate::stream::StreamEventStream;
use crate::types::Request;

pub type CompleteHandler =
    Arc<dyn Fn(Request) -> BoxFuture<'static, Result<Response, SDKError>> + Send + Sync>;
pub type StreamHandler =
    Arc<dyn Fn(Request) -> BoxFuture<'static, Result<StreamEventStream, SDKError>> + Send + Sync>;

/// Middleware for wrapping complete() and stream() calls.
#[async_trait]
pub trait Middleware: Send + Sync {
    async fn handle_complete(
        &self,
        request: Request,
        next: CompleteHandler,
    ) -> Result<Response, SDKError>;

    async fn handle_stream(
        &self,
        request: Request,
        next: StreamHandler,
    ) -> Result<StreamEventStream, SDKError>;
}

#[derive(Clone, Default)]
pub struct Client {
    providers: HashMap<String, Arc<dyn ProviderAdapter>>,
    default_provider: Option<String>,
    middleware: Vec<Arc<dyn Middleware>>,
}

impl Client {
    pub fn new(
        providers: HashMap<String, Arc<dyn ProviderAdapter>>,
        default_provider: Option<String>,
        middleware: Vec<Arc<dyn Middleware>>,
    ) -> Self {
        Self {
            providers,
            default_provider,
            middleware,
        }
    }

    pub fn register_provider(
        &mut self,
        provider: Arc<dyn ProviderAdapter>,
    ) -> Result<(), SDKError> {
        provider.initialize()?;
        let name = provider.name().to_string();
        if self.default_provider.is_none() {
            self.default_provider = Some(name.clone());
        }
        self.providers.insert(name, provider);
        Ok(())
    }

    pub fn set_default_provider(&mut self, provider: impl Into<String>) {
        self.default_provider = Some(provider.into());
    }

    pub fn add_middleware(&mut self, middleware: Arc<dyn Middleware>) {
        self.middleware.push(middleware);
    }

    pub fn from_env() -> Result<Self, SDKError> {
        crate::anthropic::ensure_anthropic_factory_registered();
        crate::openai::ensure_openai_factory_registered();

        let mut providers = HashMap::new();
        let mut default_provider = None;

        for factory in registered_factories() {
            if let Some(adapter) = factory.from_env() {
                adapter.initialize()?;
                let name = adapter.name().to_string();
                if default_provider.is_none() {
                    default_provider = Some(name.clone());
                }
                providers.insert(name, adapter);
            }
        }

        if providers.is_empty() {
            return Err(SDKError::Configuration(ConfigurationError::new(
                "no providers configured from environment",
            )));
        }

        Ok(Self {
            providers,
            default_provider,
            middleware: Vec::new(),
        })
    }

    pub async fn complete(&self, mut request: Request) -> Result<Response, SDKError> {
        let provider_name = self.resolve_provider(&request)?;
        request.provider = Some(provider_name.clone());
        let adapter = self
            .providers
            .get(&provider_name)
            .ok_or_else(|| {
                SDKError::Configuration(ConfigurationError::new("provider not registered"))
            })?
            .clone();

        let base: CompleteHandler = Arc::new(move |req| {
            let adapter = adapter.clone();
            Box::pin(async move { adapter.complete(req).await })
        });

        let handler = self.middleware.iter().rev().fold(base, |next, middleware| {
            let middleware = middleware.clone();
            Arc::new(move |req| {
                let middleware = middleware.clone();
                let next = next.clone();
                Box::pin(async move { middleware.handle_complete(req, next).await })
            })
        });

        handler(request).await
    }

    pub async fn stream(&self, mut request: Request) -> Result<StreamEventStream, SDKError> {
        let provider_name = self.resolve_provider(&request)?;
        request.provider = Some(provider_name.clone());
        let adapter = self
            .providers
            .get(&provider_name)
            .ok_or_else(|| {
                SDKError::Configuration(ConfigurationError::new("provider not registered"))
            })?
            .clone();

        let base: StreamHandler = Arc::new(move |req| {
            let adapter = adapter.clone();
            Box::pin(async move { adapter.stream(req).await })
        });

        let handler = self.middleware.iter().rev().fold(base, |next, middleware| {
            let middleware = middleware.clone();
            Arc::new(move |req| {
                let middleware = middleware.clone();
                let next = next.clone();
                Box::pin(async move { middleware.handle_stream(req, next).await })
            })
        });

        handler(request).await
    }

    pub fn close(&self) -> Result<(), SDKError> {
        for adapter in self.providers.values() {
            adapter.close()?;
        }
        Ok(())
    }

    fn resolve_provider(&self, request: &Request) -> Result<String, SDKError> {
        if let Some(provider) = &request.provider {
            return Ok(provider.clone());
        }
        if let Some(provider) = &self.default_provider {
            return Ok(provider.clone());
        }
        Err(SDKError::Configuration(ConfigurationError::new(
            "no provider configured",
        )))
    }
}

static DEFAULT_CLIENT: OnceLock<RwLock<Option<Arc<Client>>>> = OnceLock::new();

fn default_client_store() -> &'static RwLock<Option<Arc<Client>>> {
    DEFAULT_CLIENT.get_or_init(|| RwLock::new(None))
}

/// Get the module-level default client, initializing from environment variables.
pub fn default_client() -> Result<Arc<Client>, SDKError> {
    {
        let store = default_client_store()
            .read()
            .expect("default client store poisoned");
        if let Some(client) = &*store {
            return Ok(client.clone());
        }
    }

    let client = Client::from_env()?;
    let mut store = default_client_store()
        .write()
        .expect("default client store poisoned");
    if store.is_none() {
        *store = Some(Arc::new(client));
    }

    store.clone().ok_or_else(|| {
        SDKError::Configuration(ConfigurationError::new("default client unavailable"))
    })
}

/// Override the module-level default client.
pub fn set_default_client(client: Client) -> Result<(), SDKError> {
    let mut store = default_client_store()
        .write()
        .expect("default client store poisoned");
    *store = Some(Arc::new(client));
    Ok(())
}

#[cfg(test)]
pub(crate) fn clear_default_client_for_tests() {
    let mut store = default_client_store()
        .write()
        .expect("default client store poisoned");
    *store = None;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{
        ProviderFactory, clear_provider_factories_for_tests, register_provider_factory,
    };
    use crate::stream::{StreamEvent, StreamEventStream, StreamEventType, StreamEventTypeOrString};
    use crate::types::{Message, Usage};
    use futures::stream;
    use std::sync::{Mutex, MutexGuard};

    struct TestAdapter {
        name: String,
    }

    #[async_trait]
    impl ProviderAdapter for TestAdapter {
        fn name(&self) -> &str {
            &self.name
        }

        async fn complete(&self, _request: Request) -> Result<Response, SDKError> {
            Ok(Response {
                id: "resp".to_string(),
                model: "model".to_string(),
                provider: self.name.clone(),
                message: Message::assistant("ok"),
                finish_reason: crate::types::FinishReason {
                    reason: "stop".to_string(),
                    raw: None,
                },
                usage: Usage {
                    input_tokens: 0,
                    output_tokens: 0,
                    total_tokens: 0,
                    reasoning_tokens: None,
                    cache_read_tokens: None,
                    cache_write_tokens: None,
                    raw: None,
                },
                raw: None,
                warnings: vec![],
                rate_limit: None,
            })
        }

        async fn stream(&self, _request: Request) -> Result<StreamEventStream, SDKError> {
            let event = StreamEvent {
                event_type: StreamEventTypeOrString::Known(StreamEventType::Finish),
                delta: None,
                text_id: None,
                reasoning_delta: None,
                tool_call: None,
                finish_reason: None,
                usage: None,
                response: None,
                error: None,
                raw: None,
            };
            Ok(Box::pin(stream::iter(vec![Ok(event)])))
        }
    }

    struct StaticFactory {
        id: &'static str,
        adapter_name: &'static str,
        enabled: bool,
    }

    impl ProviderFactory for StaticFactory {
        fn provider_id(&self) -> &'static str {
            self.id
        }

        fn from_env(&self) -> Option<Arc<dyn ProviderAdapter>> {
            if self.enabled {
                Some(Arc::new(TestAdapter {
                    name: self.adapter_name.to_string(),
                }))
            } else {
                None
            }
        }
    }

    struct OrderMiddleware {
        label: &'static str,
        log: Arc<Mutex<Vec<&'static str>>>,
    }

    #[async_trait]
    impl Middleware for OrderMiddleware {
        async fn handle_complete(
            &self,
            request: Request,
            next: CompleteHandler,
        ) -> Result<Response, SDKError> {
            self.log.lock().unwrap().push(self.label);
            let result = next(request).await;
            self.log.lock().unwrap().push(self.label);
            result
        }

        async fn handle_stream(
            &self,
            request: Request,
            next: StreamHandler,
        ) -> Result<StreamEventStream, SDKError> {
            self.log.lock().unwrap().push(self.label);
            let result = next(request).await;
            self.log.lock().unwrap().push(self.label);
            result
        }
    }

    static TEST_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    fn global_test_guard() -> MutexGuard<'static, ()> {
        TEST_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .expect("test lock poisoned")
    }

    fn base_request() -> Request {
        Request {
            model: "model".to_string(),
            messages: vec![Message::user("hi")],
            provider: None,
            tools: None,
            tool_choice: None,
            response_format: None,
            temperature: None,
            top_p: None,
            max_tokens: None,
            stop_sequences: None,
            reasoning_effort: None,
            metadata: None,
            provider_options: None,
        }
    }

    fn build_client_with_provider(name: &str) -> Client {
        let adapter = Arc::new(TestAdapter {
            name: name.to_string(),
        });
        let mut providers: HashMap<String, Arc<dyn ProviderAdapter>> = HashMap::new();
        providers.insert(name.to_string(), adapter);
        Client::new(providers, Some(name.to_string()), vec![])
    }

    #[tokio::test(flavor = "current_thread")]
    async fn middleware_order_is_preserved() {
        let adapter = Arc::new(TestAdapter {
            name: "test".to_string(),
        });
        let mut providers: HashMap<String, Arc<dyn ProviderAdapter>> = HashMap::new();
        providers.insert("test".to_string(), adapter);
        let mut client = Client::new(providers, Some("test".to_string()), vec![]);

        let log = Arc::new(Mutex::new(Vec::new()));
        client.add_middleware(Arc::new(OrderMiddleware {
            label: "a",
            log: log.clone(),
        }));
        client.add_middleware(Arc::new(OrderMiddleware {
            label: "b",
            log: log.clone(),
        }));

        let _ = client.complete(base_request()).await.unwrap();
        let order = log.lock().unwrap().clone();
        assert_eq!(order, vec!["a", "b", "b", "a"]);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn provider_resolution_prefers_request_provider() {
        let adapter = Arc::new(TestAdapter {
            name: "test".to_string(),
        });
        let mut client = Client::new(HashMap::new(), Some("test".to_string()), vec![]);
        client.register_provider(adapter).unwrap();

        let mut request = base_request();
        request.provider = Some("test".to_string());
        let response = client.complete(request).await.unwrap();
        assert_eq!(response.provider, "test");
    }

    #[test]
    fn from_env_registers_provider_and_sets_default_in_registration_order() {
        let _guard = global_test_guard();
        clear_provider_factories_for_tests();
        clear_default_client_for_tests();
        register_provider_factory(Arc::new(StaticFactory {
            id: "provider-b",
            adapter_name: "provider_b",
            enabled: true,
        }));
        register_provider_factory(Arc::new(StaticFactory {
            id: "provider-a",
            adapter_name: "provider_a",
            enabled: true,
        }));
        let client = Client::from_env().unwrap();
        assert_eq!(client.default_provider.as_deref(), Some("provider_b"));
        assert!(client.providers.contains_key("provider_b"));
        assert!(client.providers.contains_key("provider_a"));
    }

    #[test]
    fn from_env_fails_fast_when_no_provider_is_available() {
        let _guard = global_test_guard();
        clear_provider_factories_for_tests();
        clear_default_client_for_tests();
        register_provider_factory(Arc::new(StaticFactory {
            id: "provider-disabled",
            adapter_name: "provider_disabled",
            enabled: false,
        }));
        let result = Client::from_env();
        assert!(matches!(result, Err(SDKError::Configuration(_))));
    }

    #[test]
    fn set_default_client_overrides_runtime_default_client() {
        let _guard = global_test_guard();
        clear_provider_factories_for_tests();
        clear_default_client_for_tests();

        set_default_client(build_client_with_provider("first")).unwrap();
        let first = default_client().unwrap();
        assert_eq!(first.default_provider.as_deref(), Some("first"));

        set_default_client(build_client_with_provider("second")).unwrap();
        let second = default_client().unwrap();
        assert_eq!(second.default_provider.as_deref(), Some("second"));
    }

    #[test]
    fn default_client_lazily_initializes_from_env() {
        let _guard = global_test_guard();
        clear_provider_factories_for_tests();
        clear_default_client_for_tests();
        register_provider_factory(Arc::new(StaticFactory {
            id: "provider-env",
            adapter_name: "env_provider",
            enabled: true,
        }));

        let client = default_client().unwrap();
        assert_eq!(client.default_provider.as_deref(), Some("env_provider"));
        assert!(client.providers.contains_key("env_provider"));
    }
}
