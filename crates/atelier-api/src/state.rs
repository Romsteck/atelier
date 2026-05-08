use std::sync::Arc;

#[derive(Clone, Default)]
pub struct ApiState {
    pub inner: Arc<Inner>,
}

#[derive(Default)]
pub struct Inner {}

impl ApiState {
    pub fn new() -> Self {
        Self::default()
    }
}
