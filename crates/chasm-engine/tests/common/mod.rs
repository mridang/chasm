//! Shared fixtures across engine integration test binaries.

use chasm_engine::MockRequest;
use std::collections::HashMap;

/// Builds a default request shell with the given method and path.
pub fn req(method: &str, path: &str) -> MockRequest {
    {
        let mut __r = MockRequest::default();
        __r.method = method.to_string();
        __r.path = path.to_string();
        __r.headers = HashMap::new();
        __r.query = HashMap::new();
        __r.body = None;
        __r
    }
}
