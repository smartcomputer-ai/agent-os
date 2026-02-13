use crate::contracts::{SessionEvent, SessionState};

/// Extension hooks for SDK-based session reducers.
pub trait SessionReducerHooks {
    type Error;

    fn before_event(
        &mut self,
        _state: &SessionState,
        _event: &SessionEvent,
    ) -> Result<(), Self::Error> {
        Ok(())
    }

    fn after_event(
        &mut self,
        _state: &SessionState,
        _event: &SessionEvent,
    ) -> Result<(), Self::Error> {
        Ok(())
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct NoopSessionHooks;

impl SessionReducerHooks for NoopSessionHooks {
    type Error = core::convert::Infallible;
}
